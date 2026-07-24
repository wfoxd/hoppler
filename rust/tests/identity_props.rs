//! Property tests for the identity subsystem (T05): the three pseudonym
//! invariants the block guarantee depends on, and persona-record robustness.

use proptest::prelude::*;
use rust_lib_hoppler::crypto::sign;
use rust_lib_hoppler::identity::{verify_persona_record, Identity, Persona};

fn identity_from(l1: [u8; 32], l2: [u8; 32], name: String, colour: u32) -> Identity {
    Identity::from_parts(
        &l1,
        &l2,
        Persona {
            name,
            colour,
            version: 1,
        },
    )
}

proptest! {
    // Determinism: the pseudonym toward a fixed counterpart is a pure function
    // of our Layer-1 seed and their Layer-2 public.
    #[test]
    fn pseudonym_is_deterministic(l1 in any::<[u8; 32]>(), peer in any::<[u8; 32]>()) {
        let me = identity_from(l1, [7u8; 32], "me".into(), 0);
        let peer_l2 = sign::SigningKeyPair::from_seed(&peer).public();
        prop_assert_eq!(me.pseudonym_toward(&peer_l2).0, me.pseudonym_toward(&peer_l2).0);
    }

    // Unlinkability across counterparts: two distinct peers get distinct
    // pseudonyms from the same sender.
    #[test]
    fn pseudonym_distinct_across_distinct_peers(l1 in any::<[u8; 32]>(), a in any::<[u8; 32]>(), b in any::<[u8; 32]>()) {
        prop_assume!(a != b);
        let me = identity_from(l1, [7u8; 32], "me".into(), 0);
        let pa = sign::SigningKeyPair::from_seed(&a).public();
        let pb = sign::SigningKeyPair::from_seed(&b).public();
        prop_assert_ne!(me.pseudonym_toward(&pa).0, me.pseudonym_toward(&pb).0);
    }

    // Own-Layer-2 independence: rotating our Layer-2 leaves outbound pseudonyms
    // unchanged (they key on our Layer-1, not our Layer-2).
    #[test]
    fn outbound_pseudonym_independent_of_own_layer2(
        l1 in any::<[u8; 32]>(),
        l2a in any::<[u8; 32]>(),
        l2b in any::<[u8; 32]>(),
        peer in any::<[u8; 32]>(),
    ) {
        let peer_l2 = sign::SigningKeyPair::from_seed(&peer).public();
        let before = identity_from(l1, l2a, "before".into(), 0);
        let after = identity_from(l1, l2b, "after".into(), 0);
        prop_assert_eq!(before.pseudonym_toward(&peer_l2).0, after.pseudonym_toward(&peer_l2).0);
    }

    // A record generated from arbitrary persona fields always verifies back to
    // exactly those fields.
    #[test]
    fn persona_record_round_trips(name in ".{0,64}", colour in any::<u32>()) {
        let id = identity_from([1u8; 32], [2u8; 32], name.clone(), colour);
        let verified = verify_persona_record(&id.persona_record()).unwrap();
        prop_assert_eq!(verified.name, name);
        prop_assert_eq!(verified.colour, colour);
        prop_assert_eq!(verified.l2_pub.0, id.layer2_public().0);
    }

    // Verification never panics on arbitrary bytes.
    #[test]
    fn verify_never_panics(wire in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = verify_persona_record(&wire);
    }
}
