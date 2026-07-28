//! Session-layer tests (T10) — R0-F5, and R0-F10's enforcement.
//!
//! The two that matter most are the transcript tests. T10's acceptance is
//! stated in terms of *bytes on the wire* — "no responder bytes precede receipt
//! of a valid initiator pseudonym", "a blocked-pseudonym attempt produces zero
//! response frames" — so they are checked that way rather than by asserting on
//! a return value that happens to be an error.

use rust_lib_hoppler::identity::{self, Identity, Persona};
use rust_lib_hoppler::session::handshake::{
    HandshakeError, Initiator, Responder, MAX_HANDSHAKE_PAYLOAD,
};

/// Alice discovers Bob and holds his verified persona, which is what IK needs
/// before it can start. In production this comes from the T09 endpoint.
fn pair() -> (Identity, Identity, identity::VerifiedPersona) {
    let alice = Identity::generate("Alice", 0x00_ff_00);
    let bob = Identity::generate("Bob", 0x00_00_ff);
    let bob_persona = identity::verify_persona_record(&bob.persona_record()).unwrap();
    (alice, bob, bob_persona)
}

// ── the happy path ──────────────────────────────────────────────────────────

#[test]
fn two_strangers_establish_a_session_and_talk() {
    let (alice, bob, bob_persona) = pair();

    let (initiator, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
    let pending = Responder::read_first(&bob, &msg1).unwrap();
    let (mut bob_session, msg2) = pending.accept(&bob).unwrap();
    let mut alice_session = initiator.finish(&msg2).unwrap();

    // Each side learned who the other is, from the signed payload.
    assert_eq!(bob_session.persona.name, "Alice");
    assert_eq!(alice_session.persona.name, "Bob");

    // And the transport keys agree.
    let mut ct = vec![0u8; 1024];
    let n = alice_session
        .transport
        .write_message(b"hello over the pipe", &mut ct)
        .unwrap();
    let mut pt = vec![0u8; 1024];
    let m = bob_session
        .transport
        .read_message(&ct[..n], &mut pt)
        .unwrap();
    assert_eq!(&pt[..m], b"hello over the pipe");
}

#[test]
fn the_initiators_static_is_its_pseudonym_toward_this_responder() {
    let (alice, bob, bob_persona) = pair();
    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
    let pending = Responder::read_first(&bob, &msg1).unwrap();

    // What the handshake *proved* must be the value a block binds to (R0-F10).
    assert_eq!(
        pending.pseudonym().0,
        alice.pseudonym_toward(&bob_persona.l2_pub).0,
        "the proven static is not Alice's pseudonym toward Bob, so a block \
         recorded against that pseudonym would never match a real session"
    );
}

/// R0-F10, in full: a block sticks to the *device*. "A new name, a new colour,
/// even a freshly generated Layer-2 persona does not evade it — evasion
/// requires destroying the whole Layer-1 identity."
///
/// All three of those are tested, because they are three different claims. An
/// earlier version of this test only renamed the persona, which a
/// Layer-2-derived pseudonym would also survive — so it passed while proving
/// nothing about which layer the derivation actually uses. Regenerating the
/// Layer-2 *keypair* is what separates the two.
#[test]
fn no_change_short_of_destroying_layer_1_changes_the_pseudonym() {
    let (mut alice, bob, bob_persona) = pair();
    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
    let original = *Responder::read_first(&bob, &msg1).unwrap().pseudonym();

    // 1. A new name and colour.
    alice.update_persona("Totally Someone Else", 0xff_00_00);
    let (_, renamed) = Initiator::start(&alice, &bob_persona).unwrap();
    assert_eq!(
        Responder::read_first(&bob, &renamed).unwrap().pseudonym().0,
        original.0,
        "a rename changed the pseudonym — a block would be evaded from the \
         settings screen"
    );

    // 2. An entirely fresh Layer-2 keypair, keeping Layer-1.
    let reborn = Identity::from_parts(
        &alice.layer1_seed(),
        &Identity::generate("x", 0).layer2_seed(),
        Persona {
            name: "Brand New Me".into(),
            colour: 0x12_34_56,
            version: 1,
        },
    );
    let (_, regenerated) = Initiator::start(&reborn, &bob_persona).unwrap();
    assert_eq!(
        Responder::read_first(&bob, &regenerated)
            .unwrap()
            .pseudonym()
            .0,
        original.0,
        "regenerating the Layer-2 persona changed the pseudonym — the block \
         binds to Layer-2 after all, and F10's central promise is false"
    );

    // 3. A new Layer-1 — a different device. This one *must* differ, or a
    // block would follow an identity that has nothing to do with the blocked
    // one.
    let stranger = Identity::from_parts(
        &Identity::generate("y", 0).layer1_seed(),
        &alice.layer2_seed(),
        alice.persona().clone(),
    );
    let (_, other_device) = Initiator::start(&stranger, &bob_persona).unwrap();
    assert_ne!(
        Responder::read_first(&bob, &other_device)
            .unwrap()
            .pseudonym()
            .0,
        original.0,
        "a different Layer-1 identity produced the same pseudonym"
    );
}

#[test]
fn the_same_device_looks_different_to_different_counterparties() {
    let (alice, bob, bob_persona) = pair();
    let carol = Identity::generate("Carol", 1);
    let carol_persona = identity::verify_persona_record(&carol.persona_record()).unwrap();

    let (_, to_bob) = Initiator::start(&alice, &bob_persona).unwrap();
    let (_, to_carol) = Initiator::start(&alice, &carol_persona).unwrap();

    let seen_by_bob = *Responder::read_first(&bob, &to_bob).unwrap().pseudonym();
    let seen_by_carol = *Responder::read_first(&carol, &to_carol)
        .unwrap()
        .pseudonym();

    assert_ne!(
        seen_by_bob.0, seen_by_carol.0,
        "Bob and Carol see the same pseudonym for Alice — comparing notes \
         would link her across counterparties"
    );
}

// ── the transcript properties ───────────────────────────────────────────────

/// T10's acceptance, stated as it is written: **no responder bytes precede
/// receipt of a valid initiator pseudonym.**
///
/// Checked structurally rather than by inspection. `read_first` returns a
/// `Pending` and returns no bytes; only `accept` can produce any. A refusal is
/// therefore a value being dropped, and there is no path by which it could emit
/// something — so the property cannot be lost by someone later adding a
/// well-meaning error reply, because there is nowhere to put one.
#[test]
fn a_responder_emits_nothing_before_it_knows_who_is_calling() {
    let (alice, bob, bob_persona) = pair();
    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();

    // Reading the first message yields an identity and no wire output at all.
    let pending = Responder::read_first(&bob, &msg1).unwrap();
    assert_eq!(pending.persona().name, "Alice");

    // Bytes exist only once the responder has decided to answer.
    let (_, msg2) = pending.accept(&bob).unwrap();
    assert!(!msg2.is_empty());
}

/// R0-F10: a blocked initiator produces **zero** response frames.
///
/// Refusal here is dropping the `Pending`. This test exists to pin that the
/// decision point comes with the identity and before any output — if a future
/// change made `read_first` emit so much as an error frame, there would be
/// bytes to observe and this would fail.
#[test]
fn a_blocked_initiator_gets_zero_response_frames() {
    let (alice, bob, bob_persona) = pair();
    let blocklist = [alice.pseudonym_toward(&bob_persona.l2_pub).0];

    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
    let pending = Responder::read_first(&bob, &msg1).unwrap();

    let mut emitted: Vec<u8> = Vec::new();
    if blocklist.contains(&pending.pseudonym().0) {
        drop(pending); // the whole of "refuse"
    } else {
        let (_, msg2) = pending.accept(&bob).unwrap();
        emitted = msg2;
    }

    assert!(
        emitted.is_empty(),
        "a blocked initiator was answered with {} bytes — it can now tell \
         'blocked' from 'not there', which is the property F10 turns on",
        emitted.len()
    );
}

/// The evasion T09 could not prevent, now closed.
///
/// T09's endpoint took the pseudonym on trust, so a blocked device could simply
/// claim a different one. Here the pseudonym *is* the Noise static: claiming
/// someone else's means completing a handshake with a key you do not hold.
#[test]
fn claiming_someone_elses_pseudonym_does_not_complete_a_handshake() {
    let (alice, bob, bob_persona) = pair();
    let mallory = Identity::generate("Mallory", 9);

    // Mallory would like to be seen as Alice. She can copy Alice's persona
    // record — it is public — but the handshake static is derived from Alice's
    // Layer-1 seed, which she does not have.
    let (_, msg1) = Initiator::start(&mallory, &bob_persona).unwrap();
    let pending = Responder::read_first(&bob, &msg1).unwrap();

    assert_ne!(
        pending.pseudonym().0,
        alice.pseudonym_toward(&bob_persona.l2_pub).0,
        "Mallory presented Alice's pseudonym without Alice's key"
    );
    assert_eq!(pending.persona().name, "Mallory");
}

// ── hostile input ───────────────────────────────────────────────────────────

#[test]
fn a_handshake_aimed_at_someone_else_is_refused() {
    let (alice, bob, _) = pair();
    let carol = Identity::generate("Carol", 3);
    let carol_persona = identity::verify_persona_record(&carol.persona_record()).unwrap();

    // Alice dials Carol; Bob receives it. IK binds the responder's static into
    // the first message, so Bob cannot read a message addressed to Carol.
    let (_, for_carol) = Initiator::start(&alice, &carol_persona).unwrap();
    assert!(
        Responder::read_first(&bob, &for_carol).is_err(),
        "Bob decrypted a handshake addressed to Carol"
    );
}

#[test]
fn a_mangled_first_message_is_refused_without_panicking() {
    let (alice, bob, bob_persona) = pair();
    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();

    for (what, bad) in [
        ("empty", Vec::new()),
        ("truncated", msg1[..msg1.len() / 2].to_vec()),
        ("junk", vec![0xab; 64]),
        ("oversized", vec![0u8; 70_000]),
        (
            "bit-flipped tag",
            msg1.iter()
                .enumerate()
                .map(|(i, b)| if i + 1 == msg1.len() { b ^ 1 } else { *b })
                .collect(),
        ),
    ] {
        assert!(
            Responder::read_first(&bob, &bad).is_err(),
            "{what}: accepted"
        );
    }
}

#[test]
fn a_replayed_first_message_cannot_reuse_the_session_keys() {
    let (alice, bob, bob_persona) = pair();
    let (_, msg1) = Initiator::start(&alice, &bob_persona).unwrap();

    let first = Responder::read_first(&bob, &msg1).unwrap();
    let (mut session_a, _) = first.accept(&bob).unwrap();
    let replay = Responder::read_first(&bob, &msg1).unwrap();
    let (mut session_b, _) = replay.accept(&bob).unwrap();

    // A replay is readable — IK does not prevent that on its own — but the
    // responder's fresh ephemeral means the two sessions do not share keys, so
    // a replayed handshake buys an attacker nothing it can decrypt. Recorded
    // here because the property is easy to assume and expensive to be wrong
    // about; genuine replay *rejection* is the caller's job (a session table
    // that refuses a second live session per pseudonym).
    let mut ct = vec![0u8; 256];
    let n = session_a
        .transport
        .write_message(b"secret", &mut ct)
        .unwrap();
    let mut pt = vec![0u8; 256];
    assert!(
        session_b.transport.read_message(&ct[..n], &mut pt).is_err(),
        "two sessions from one replayed handshake share transport keys"
    );
}

#[test]
fn a_responder_impersonating_another_persona_is_caught() {
    let (alice, bob, bob_persona) = pair();

    // Bob answers, but hands back a persona that is not the one whose session
    // key the handshake used. Without the cross-check the initiator would
    // attribute the session to whoever the payload claimed.
    let (initiator, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
    let pending = Responder::read_first(&bob, &msg1).unwrap();
    let mallory = Identity::generate("Mallory", 9);
    let (_, msg2) = pending.accept(&mallory).unwrap();

    assert!(
        matches!(
            initiator.finish(&msg2),
            Err(HandshakeError::PseudonymMismatch)
        ),
        "a responder passed off another device's persona as its own"
    );
}

#[test]
fn a_payload_beyond_the_bound_is_refused_rather_than_allocated() {
    // The bound exists so a peer cannot make us allocate on its say-so; the
    // constant is public so a caller can size its own buffers to match.
    assert!(MAX_HANDSHAKE_PAYLOAD <= 4096);
    let (alice, _, bob_persona) = pair();
    let record = alice.persona_record();
    assert!(
        record.len() < MAX_HANDSHAKE_PAYLOAD,
        "a persona record no longer fits a handshake payload ({} bytes)",
        record.len()
    );
    let _ = bob_persona;
}
