# The BLE platform channel — v1

The contract between the Rust core and a native BLE adapter. One document, one
version number; the Dart client and the Kotlin adapter both cite it and neither
gets to reinterpret it.

It exists because the adapter is the one component in Hoppler that no Rust test
can reach. Everything written here is either checkable in Rust (and therefore
not the adapter's problem) or is a promise the adapter alone can keep — and the
second list is deliberately short.

**Version 1.** Bumped on any change to a method name, an argument, an event
shape, or an error code. The adapter reports its version through `version`; a
mismatch is fatal at startup rather than a mystery at runtime.

---

## 1. Shape

Two channels, named after the Rust seam they carry
([`BlePlatform`](../rust/src/transport/ble.rs)):

| Channel | Name | Direction |
|---|---|---|
| Method | `org.hoppler/ble` | core → adapter (commands) |
| Event | `org.hoppler/ble/events` | adapter → core (facts) |

The asymmetry is the design. Commands are requests the radio may refuse; facts
are things that already happened and cannot be refused. Nothing flows the other
way on either channel.

## 2. Commands

Each maps 1:1 to a `BlePlatform` method and to one Android API call. Arguments
are a `Map<String, Object>`; a command returns `null` on success or raises a
`PlatformException` (§4).

| Method | Arguments | Meaning |
|---|---|---|
| `version` | — | Returns `int`. Must equal 1. |
| `startAdvertising` | `localId: String`, `payload: Uint8List` | Advertise `payload` under `localId`, replacing any current advertisement. |
| `stopAdvertising` | — | Stop. Must be a radio stop, not a flag (§5.1). |
| `startScanning` | — | Begin scanning. |
| `stopScanning` | — | Stop scanning. Open pipes are unaffected. |
| `connect` | `peer: String` | Open an L2CAP CoC (GATT fallback) to `peer`. **Acceptance only** — success arrives as `pipeOpened`, failure as `pipeFailed`. |
| `send` | `peer: String`, `bytes: Uint8List` | Write to an open pipe, preserving order and contiguity (§5.2). |
| `disconnect` | `peer: String` | Close one pipe. |
| `shutdown` | — | Release every radio resource. Late events afterwards are tolerated. |

## 3. Events

Each event is a `Map` with a `type` key. Unknown types are ignored by the core,
so a newer adapter may emit events an older core has never heard of.

| `type` | Fields | Maps to |
|---|---|---|
| `peerFound` | `peer: String`, `payload: Uint8List` | `PlatformEvent::PeerFound` |
| `peerLost` | `peer: String` | `PlatformEvent::PeerLost` |
| `pipeOpened` | `peer: String` | `PlatformEvent::PipeOpened` |
| `pipeFailed` | `peer: String`, `why: String` | `PlatformEvent::PipeFailed` |
| `pipeClosed` | `peer: String` | `PlatformEvent::PipeClosed` |
| `received` | `peer: String`, `bytes: Uint8List` | `PlatformEvent::Received` |
| `availability` | `available: bool`, `reason: String?` | `PlatformEvent::Availability` |
| `writeComplete` | `peer: String`, `bytes: int` | `BleIngress::on_write_complete` |

## 4. Errors

`PlatformException.code` comes from this closed set and nothing else. The
message is for logs; the core routes on the code.

| Code | Becomes | Use when |
|---|---|---|
| `unavailable` | `TransportError::Unavailable` | Bluetooth off, permission denied, adapter not ready |
| `no_such_peer` | `TransportError::NoSuchPeer` | No pipe or no such device |
| `payload_too_large` | `TransportError::PayloadTooLarge` | Advertisement exceeds what the radio accepts |
| `would_block` | `TransportError::WouldBlock` | Radio buffers full; the core will retry |
| `io` | `TransportError::Io` | Anything else at the link layer |

An unrecognised code is treated as `io`.

## 5. What the adapter must get right

The whole list. Everything else the contract promises is decided in Rust.

### 5.1 `stopAdvertising` must actually stop the radio

R0-F2's guarantee is that Discovery off is *really* off. Returning success for a
stop the radio refused reports invisibility that does not exist, and no test
above this layer can detect it. If the stop fails, raise — do not swallow.

### 5.2 One `send` stays one contiguous, ordered write

The core serialises per pipe and hands over one buffer at a time. The adapter
may fragment to fit the MTU, but the fragments must reach the peer in order and
must not interleave with another `send`'s. The layer above frames its own
messages and will desync — visibly as a crypto failure — if bytes are reordered.

### 5.3 `writeComplete` must follow every accepted `send`

The core meters outstanding bytes per pipe and stops sending when the window is
full; the credit is returned only by `writeComplete`. Omit it and the pipe
silently wedges after ~64 kB. Report the byte count that actually reached the
radio.

### 5.4 A `peer` is a stable, valid id for the life of a pipe

Ids must match the core's rule — ASCII alphanumeric and `-`, 1–63 bytes, no
leading or trailing `-` — because they are namespaced across rungs (`ble:1f3a`)
and a `.` or `:` splits differently on the two ends of one pipe.

Derive the id from the *advertisement*, never from the MAC address: Android
rotates the address underneath you, and a peer that changes id mid-pipe breaks
contract rule 4. The same peer may legitimately appear under a new id after a
rotation, with no pipe open — report that as `peerLost` then `peerFound`.

### 5.5 Facts, not interpretations

Report what the radio did. Do not suppress a duplicate, infer a close from a
timeout, or re-order events to look tidier — the core has more context and
handles all three (§6).

## 6. What the adapter is *not* responsible for

Stated explicitly, because every item here is one an adapter author would
otherwise feel obliged to implement, in Kotlin, untested:

- **Re-announcing an already-open pipe.** `connect` on an open pipe still emits
  `PipeOpened` to the core — decided in Rust.
- **Duplicate `pipeOpened`.** Both ends dialling simultaneously is normal on
  BLE. Report both; the core opens one pipe.
- **Silence after `shutdown`.** Late events are expected and discarded.
- **`pipeClosed` for a pipe that never opened.** Report it; the core turns it
  into `PipeFailed`.
- **Bytes arriving after the core hung up.** Report them; they are dropped.
- **Refusing to rotate the local id while a pipe is open.** Enforced in Rust.
- **Backpressure policy.** The adapter reports `writeComplete`; the core decides
  when to stop.
- **Reconnection, rotation cadence, who to dial.** Core policy (T09/T10).

## 7. Threading

Events may be emitted from any thread; the core hands them to its own dispatch
thread and never calls back into the adapter while handling one. The adapter
**may** call `writeComplete` synchronously from inside `send` — the core is
explicitly tested for that case and must not deadlock.

## 8. Permissions and lifecycle

Android 12+ requires `BLUETOOTH_ADVERTISE`, `BLUETOOTH_SCAN` and
`BLUETOOTH_CONNECT`; below that, location permission gates scanning. The adapter
does not prompt — the app does — but it must report the resulting state as
`availability` with a reason, so the UI can say "Bluetooth is off" instead of
showing an empty list that reads as "nobody is nearby" (R0-F2).

Ring 0 makes no background-delivery promise (R0-N6). The adapter may stop the
radio when the app backgrounds; it must report that as `availability`, and it
must not crash.
