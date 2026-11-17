//! Replays the shared vector files with snow, in both roles.
use s04_noise_interop::*;
use snow::Builder;
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vectors")
}

fn load_vector_file(name: &str) -> VectorFile {
    let text = std::fs::read_to_string(vectors_dir().join(name))
        .unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn load_published() -> PublishedFile {
    let text = std::fs::read_to_string(vectors_dir().join("published-ik.json"))
        .expect("read published-ik.json");
    serde_json::from_str(&text).expect("parse published-ik.json")
}

fn replay_file(file: &VectorFile) {
    assert_eq!(file.protocol, PROTOCOL);
    assert!(file.cases.len() >= 4);
    for case in &file.cases {
        let script = script_of_case(case).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        // The recorded public keys must match the DH implementation of snow's resolver.
        assert_eq!(
            case.public_keys.responder_static.bytes().unwrap(),
            public_key(&script.resp_static),
            "{}",
            case.name
        );
        assert_eq!(
            case.public_keys.initiator_static.bytes().unwrap(),
            public_key(&script.init_static),
            "{}",
            case.name
        );
        for role in [Role::Initiator, Role::Responder] {
            replay(&script, role).unwrap_or_else(|e| panic!("{} as {role:?}: {e}", case.name));
        }
    }
    assert_eq!(file.negative.len(), 2);
    for case in &file.negative {
        replay_negative(case).unwrap_or_else(|e| panic!("{}: {e}", case.name));
    }
}

#[test]
fn ts_vectors_replay_in_both_roles() {
    // JS initiator vs snow responder (role Responder) and snow initiator vs JS responder (role Initiator).
    let file = load_vector_file("monoptah-ik.ts.json");
    assert_eq!(file.generator, "ts");
    replay_file(&file);
}

#[test]
fn snow_vectors_replay_in_both_roles() {
    let file = load_vector_file("monoptah-ik.snow.json");
    assert_eq!(file.generator, "snow");
    replay_file(&file);
}

#[test]
fn snow_vectors_file_is_reproducible() {
    let fresh = serde_json::to_value(build_vectors().expect("build")).unwrap();
    let committed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(vectors_dir().join("monoptah-ik.snow.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fresh, committed,
        "run `cargo run --bin gen` and commit the result"
    );
}

#[test]
fn published_ik_vectors_pass_in_both_roles() {
    let file = load_published();
    assert!(
        file.vectors.len() >= 2,
        "expected the cacophony and snow IK vectors"
    );
    let mut sources = file
        .vectors
        .iter()
        .map(|v| v.source.clone())
        .collect::<Vec<_>>();
    sources.sort();
    assert_eq!(sources, ["cacophony", "snow"]);
    for v in &file.vectors {
        assert_eq!(v.protocol_name, PROTOCOL);
        let script = script_of_published(v).unwrap_or_else(|e| panic!("{}: {e}", v.source));
        for role in [Role::Initiator, Role::Responder] {
            replay(&script, role)
                .unwrap_or_else(|e| panic!("published {} as {role:?}: {e}", v.source));
        }
    }
}

#[test]
fn a_tampered_vector_is_detected() {
    // Guards the replay itself: flipping one ciphertext byte must fail.
    let mut file = load_vector_file("monoptah-ik.ts.json");
    let case = &mut file.cases[1];
    if let TransportStep::Message { ciphertext, .. } = &mut case.transport[0] {
        let mut bytes = ciphertext.bytes().unwrap();
        bytes[0] ^= 1;
        *ciphertext = ByteStr::from_bytes(&bytes);
    } else {
        panic!("first step is a message");
    }
    let script = script_of_case(case).unwrap();
    assert!(replay(&script, Role::Initiator).is_err());
    assert!(replay(&script, Role::Responder).is_err());
}

fn live_pair() -> (
    snow::TransportState,
    snow::TransportState,
    snow::StatelessTransportState,
) {
    let params: snow::params::NoiseParams = PROTOCOL.parse().unwrap();
    let rs = [0x13u8; 32];
    let is = [0x11u8; 32];
    let rs_pub = public_key(&rs);
    let mut i = Builder::new(params.clone())
        .local_private_key(&is)
        .unwrap()
        .remote_public_key(&rs_pub)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&[0x12u8; 32])
        .build_initiator()
        .unwrap();
    let mut r = Builder::new(params.clone())
        .local_private_key(&rs)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&[0x14u8; 32])
        .build_responder()
        .unwrap();
    let mut buf = [0u8; 65535];
    let mut out = [0u8; 65535];
    let n = i.write_message(&[], &mut buf).unwrap();
    r.read_message(&buf[..n], &mut out).unwrap();
    let n = r.write_message(&[], &mut buf).unwrap();
    i.read_message(&buf[..n], &mut out).unwrap();
    // A stateless copy of the same session (same fixed ephemerals) to produce messages at chosen nonces.
    let mut i2 = Builder::new(params.clone())
        .local_private_key(&is)
        .unwrap()
        .remote_public_key(&rs_pub)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&[0x12u8; 32])
        .build_initiator()
        .unwrap();
    let mut r2 = Builder::new(params)
        .local_private_key(&rs)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&[0x14u8; 32])
        .build_responder()
        .unwrap();
    let n = i2.write_message(&[], &mut buf).unwrap();
    r2.read_message(&buf[..n], &mut out).unwrap();
    let n = r2.write_message(&[], &mut buf).unwrap();
    i2.read_message(&buf[..n], &mut out).unwrap();
    (
        i.into_transport_mode().unwrap(),
        r.into_transport_mode().unwrap(),
        i2.into_stateless_transport_mode().unwrap(),
    )
}

/// Records snow's behaviour at the nonce limit (spec 5.1: nonce 2^64-1 is reserved and must not be used).
#[test]
fn snow_refuses_the_reserved_nonce() {
    let (_initiator, mut responder, stateless_initiator) = live_pair();
    let mut buf = [0u8; 65535];
    let mut out = [0u8; 65535];
    // Sending side (stateless API has the explicit nonce): nonce u64::MAX is refused, u64::MAX - 1 is accepted.
    let err = stateless_initiator
        .write_message(u64::MAX, b"x", &mut buf)
        .unwrap_err();
    assert!(
        matches!(
            err,
            snow::Error::State(snow::error::StateProblem::Exhausted)
        ),
        "got {err:?}"
    );
    let n = stateless_initiator
        .write_message(u64::MAX - 1, b"last allowed", &mut buf)
        .unwrap();
    // Receiving side (stateful): jump the receive nonce, the last allowed message is read, then reads are refused.
    responder.set_receiving_nonce(u64::MAX - 1);
    let m = responder.read_message(&buf[..n], &mut out).unwrap();
    assert_eq!(&out[..m], b"last allowed");
    assert_eq!(responder.receiving_nonce(), u64::MAX);
    let err = responder.read_message(&buf[..n], &mut out).unwrap_err();
    assert!(
        matches!(
            err,
            snow::Error::State(snow::error::StateProblem::Exhausted)
        ),
        "got {err:?}"
    );
    // The sending side of a stateful TransportState has no public nonce setter in snow 0.10.0: the
    // encrypt refusal is only reachable through the stateless API above.
    let (mut initiator2, _r, _s) = live_pair();
    initiator2.set_receiving_nonce(u64::MAX);
    let err = initiator2.read_message(&buf[..n], &mut out).unwrap_err();
    assert!(
        matches!(
            err,
            snow::Error::State(snow::error::StateProblem::Exhausted)
        ),
        "got {err:?}"
    );
}
