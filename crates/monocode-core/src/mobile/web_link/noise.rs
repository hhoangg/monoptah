//! The Noise handshake and transport of the web link (architecture.md 5.3 and
//! 5.4): `Noise_IK_25519_ChaChaPoly_SHA256` on `snow`. The desktop is the
//! responder, the browser is the initiator. The TypeScript twin is
//! `web/mock-desktop/src/noise.ts`; `testdata/noise_ik_vectors.json` pins both
//! to the same bytes.
use std::fmt;

use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, TransportState};

use super::frame::MAX_NOISE_PLAINTEXT;

pub const PROTOCOL: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
pub const PROLOGUE_PREFIX: &[u8] = b"monoptah-web/1";
/// The largest Noise message (spec section 3).
pub const MAX_NOISE_MESSAGE: usize = 65_535;

/// A Noise failure. The text never carries key bytes or plaintext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoiseError {
    /// Setup failed (bad key length, bad pattern).
    Setup(String),
    /// A handshake message did not authenticate or was malformed.
    Handshake(String),
    /// A transport message did not authenticate.
    Decrypt,
    /// The plaintext or message is larger than the Noise limit.
    TooLarge,
    /// The handshake is in the wrong state for the call.
    State(&'static str),
}

impl fmt::Display for NoiseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setup(m) => write!(f, "noise setup failed: {m}"),
            Self::Handshake(m) => write!(f, "noise handshake failed: {m}"),
            Self::Decrypt => write!(f, "noise message failed to authenticate"),
            Self::TooLarge => write!(f, "noise message too large"),
            Self::State(m) => write!(f, "noise state error: {m}"),
        }
    }
}

impl std::error::Error for NoiseError {}

/// `"monoptah-web/1" || desktopId(16) || deviceId(16) || channel(u16 big endian)`.
pub fn prologue(desktop_id: &[u8; 16], device_id: &[u8; 16], channel: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(PROLOGUE_PREFIX.len() + 34);
    out.extend_from_slice(PROLOGUE_PREFIX);
    out.extend_from_slice(desktop_id);
    out.extend_from_slice(device_id);
    out.extend_from_slice(&channel.to_be_bytes());
    out
}

/// Lowercase hex SHA-256 of a public key (same format as `monocode_mobile_link::fingerprint`).
pub fn fingerprint(public_key: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(public_key);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The u-coordinates (little endian) of the points of small order on
/// Curve25519, after the top bit is cleared and the value reduced mod p:
/// 0, 1, the two points of order 8, and p - 1.
const LOW_ORDER: [[u8; 32]; 7] = [
    [0; 32],
    {
        let mut one = [0u8; 32];
        one[0] = 1;
        one
    },
    [
        0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f, 0xc4,
        0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16, 0x5f, 0x49,
        0xb8, 0x00,
    ],
    [
        0x5f, 0x9c, 0x95, 0xbc, 0xa3, 0x50, 0x8c, 0x24, 0xb1, 0xd0, 0xb1, 0x55, 0x9c, 0x83, 0xef,
        0x5b, 0x04, 0x44, 0x5c, 0xc4, 0x58, 0x1c, 0x8e, 0x86, 0xd8, 0x22, 0x4e, 0xdd, 0xd0, 0x9f,
        0x11, 0x57,
    ],
    // p - 1, p and p + 1: the last two are the non-canonical forms of 0 and 1.
    p_plus(-1),
    p_plus(0),
    p_plus(1),
];

/// `p + delta` for p = 2^255 - 19, as 32 little-endian bytes.
const fn p_plus(delta: i8) -> [u8; 32] {
    let mut bytes = [0xff; 32];
    bytes[31] = 0x7f;
    bytes[0] = (0xed_i16 + delta as i16) as u8;
    bytes
}

/// Whether `key` is an X25519 point of small order, in any encoding: such a
/// "public key" makes every Diffie-Hellman result a constant that anyone can
/// compute, so a handshake under it would prove nothing. The top bit is
/// ignored, as X25519 ignores it.
pub fn is_low_order(key: &[u8; 32]) -> bool {
    let mut masked = *key;
    masked[31] &= 0x7f;
    LOW_ORDER.contains(&masked)
}

/// A fresh X25519 static key pair: `(private, public)`.
pub fn generate_static() -> Result<([u8; 32], [u8; 32]), String> {
    let params = PROTOCOL
        .parse()
        .map_err(|_| "invalid noise pattern".to_string())?;
    let pair = Builder::new(params)
        .generate_keypair()
        .map_err(|e| format!("key generation failed: {e}"))?;
    let private: [u8; 32] = pair
        .private
        .as_slice()
        .try_into()
        .map_err(|_| "unexpected private key length".to_string())?;
    let public: [u8; 32] = pair
        .public
        .as_slice()
        .try_into()
        .map_err(|_| "unexpected public key length".to_string())?;
    Ok((private, public))
}

fn builder<'a>() -> Result<Builder<'a>, NoiseError> {
    let params = PROTOCOL
        .parse()
        .map_err(|_| NoiseError::Setup("invalid noise pattern".into()))?;
    Ok(Builder::new(params))
}

fn setup(e: snow::Error) -> NoiseError {
    NoiseError::Setup(e.to_string())
}

fn handshake(e: snow::Error) -> NoiseError {
    NoiseError::Handshake(e.to_string())
}

/// The desktop side of the handshake.
pub struct Responder {
    state: HandshakeState,
}

impl Responder {
    pub fn new(
        static_private: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
    ) -> Result<Self, NoiseError> {
        Self::build(static_private, desktop_id, device_id, channel, None)
    }

    #[cfg(test)]
    fn with_ephemeral(
        static_private: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
        ephemeral: &[u8; 32],
    ) -> Result<Self, NoiseError> {
        Self::build(
            static_private,
            desktop_id,
            device_id,
            channel,
            Some(ephemeral),
        )
    }

    fn build(
        static_private: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
        ephemeral: Option<&[u8; 32]>,
    ) -> Result<Self, NoiseError> {
        let pro = prologue(desktop_id, device_id, channel);
        let mut b = builder()?
            .local_private_key(static_private)
            .map_err(setup)?
            .prologue(&pro)
            .map_err(setup)?;
        if let Some(e) = ephemeral {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        Ok(Self {
            state: b.build_responder().map_err(setup)?,
        })
    }

    /// Reads HS1: returns the decrypted payload and the initiator static key.
    pub fn read_hs1(&mut self, msg: &[u8]) -> Result<(Vec<u8>, [u8; 32]), NoiseError> {
        if msg.len() > MAX_NOISE_MESSAGE {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; msg.len()];
        let n = self.state.read_message(msg, &mut out).map_err(handshake)?;
        out.truncate(n);
        let remote: [u8; 32] = self
            .state
            .get_remote_static()
            .and_then(|k| k.try_into().ok())
            .ok_or(NoiseError::State("no remote static key after HS1"))?;
        Ok((out, remote))
    }

    /// Writes HS2 and switches to transport mode.
    pub fn write_hs2(mut self, payload: &[u8]) -> Result<(Vec<u8>, Transport), NoiseError> {
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .write_message(payload, &mut out)
            .map_err(handshake)?;
        out.truncate(n);
        let state = self.state.into_transport_mode().map_err(handshake)?;
        Ok((out, Transport::new(state)))
    }
}

/// The browser side of the handshake. Used by tests to play the device.
#[cfg(test)]
pub(crate) struct Initiator {
    state: HandshakeState,
}

#[cfg(test)]
impl Initiator {
    pub(crate) fn new(
        static_private: &[u8; 32],
        remote_static_public: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
    ) -> Result<Self, NoiseError> {
        Self::build(
            static_private,
            remote_static_public,
            desktop_id,
            device_id,
            channel,
            None,
        )
    }

    pub(crate) fn with_ephemeral(
        static_private: &[u8; 32],
        remote_static_public: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
        ephemeral: &[u8; 32],
    ) -> Result<Self, NoiseError> {
        Self::build(
            static_private,
            remote_static_public,
            desktop_id,
            device_id,
            channel,
            Some(ephemeral),
        )
    }

    fn build(
        static_private: &[u8; 32],
        remote_static_public: &[u8; 32],
        desktop_id: &[u8; 16],
        device_id: &[u8; 16],
        channel: u16,
        ephemeral: Option<&[u8; 32]>,
    ) -> Result<Self, NoiseError> {
        let pro = prologue(desktop_id, device_id, channel);
        let mut b = builder()?
            .local_private_key(static_private)
            .map_err(setup)?
            .remote_public_key(remote_static_public)
            .map_err(setup)?
            .prologue(&pro)
            .map_err(setup)?;
        if let Some(e) = ephemeral {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        Ok(Self {
            state: b.build_initiator().map_err(setup)?,
        })
    }

    pub(crate) fn write_hs1(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .write_message(payload, &mut out)
            .map_err(handshake)?;
        out.truncate(n);
        Ok(out)
    }

    pub(crate) fn read_hs2(mut self, msg: &[u8]) -> Result<(Vec<u8>, Transport), NoiseError> {
        if msg.len() > MAX_NOISE_MESSAGE {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; msg.len()];
        let n = self.state.read_message(msg, &mut out).map_err(handshake)?;
        out.truncate(n);
        let state = self.state.into_transport_mode().map_err(handshake)?;
        Ok((out, Transport::new(state)))
    }
}

/// The two CipherStates after the handshake.
pub struct Transport {
    state: TransportState,
    sent: u64,
}

impl Transport {
    fn new(state: TransportState) -> Self {
        Self { state, sent: 0 }
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if plaintext.len() > MAX_NOISE_PLAINTEXT {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; plaintext.len() + 16];
        let n = self
            .state
            .write_message(plaintext, &mut out)
            .map_err(|_| NoiseError::Decrypt)?;
        out.truncate(n);
        self.sent += 1;
        Ok(out)
    }

    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if ciphertext.len() > MAX_NOISE_MESSAGE {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; ciphertext.len()];
        let n = self
            .state
            .read_message(ciphertext, &mut out)
            .map_err(|_| NoiseError::Decrypt)?;
        out.truncate(n);
        Ok(out)
    }

    /// Rekeys the sending CipherState and restarts the sent counter.
    pub fn rekey_outgoing(&mut self) {
        self.state.rekey_outgoing();
        self.sent = 0;
    }

    /// Rekeys the receiving CipherState.
    pub fn rekey_incoming(&mut self) {
        self.state.rekey_incoming();
    }

    /// Messages sent since the last outgoing rekey.
    pub fn sent_count(&self) -> u64 {
        self.sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("testdata/noise_ik_vectors.json");

    fn unhex(s: &str) -> Vec<u8> {
        assert!(s.len().is_multiple_of(2));
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    fn arr<const N: usize>(s: &str) -> [u8; N] {
        unhex(s).try_into().unwrap()
    }

    struct V {
        root: Value,
    }

    impl V {
        fn load() -> Self {
            Self {
                root: serde_json::from_str(VECTORS).unwrap(),
            }
        }
        fn s(&self, section: &str, key: &str) -> &str {
            self.root[section][key].as_str().unwrap()
        }
        fn input(&self, key: &str) -> &str {
            self.s("inputs", key)
        }
        fn output(&self, key: &str) -> &str {
            self.s("outputs", key)
        }
        fn steps(&self, key: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
            self.root["transport"][key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    (
                        unhex(v["plaintext"].as_str().unwrap()),
                        unhex(v["ciphertext"].as_str().unwrap()),
                    )
                })
                .collect()
        }
        fn ids(&self) -> ([u8; 16], [u8; 16], u16) {
            (
                arr(self.input("desktop_id")),
                arr(self.input("device_id")),
                self.root["inputs"]["channel"].as_u64().unwrap() as u16,
            )
        }
    }

    #[test]
    fn prologue_matches_vector() {
        let v = V::load();
        let (desktop, device, channel) = v.ids();
        assert_eq!(
            prologue(&desktop, &device, channel),
            unhex(v.input("prologue"))
        );
    }

    #[test]
    fn fingerprint_matches_vector() {
        let v = V::load();
        assert_eq!(
            fingerprint(&unhex(v.input("initiator_static_public"))),
            v.output("initiator_fingerprint")
        );
        assert_eq!(
            fingerprint(&unhex(v.input("responder_static_public"))),
            v.output("responder_fingerprint")
        );
    }

    #[test]
    fn responder_reproduces_ts_bytes_through_rekey() {
        let v = V::load();
        let (desktop, device, channel) = v.ids();
        let mut r = Responder::with_ephemeral(
            &arr(v.input("responder_static_private")),
            &desktop,
            &device,
            channel,
            &arr(v.input("responder_ephemeral_private")),
        )
        .unwrap();
        let (payload, remote) = r.read_hs1(&unhex(v.output("hs1"))).unwrap();
        assert_eq!(payload, unhex(v.input("hs1_payload")));
        assert_eq!(remote.to_vec(), unhex(v.input("initiator_static_public")));
        let (hs2, mut t) = r.write_hs2(&unhex(v.input("hs2_payload"))).unwrap();
        assert_eq!(hs2, unhex(v.output("hs2")));

        // Responder sends r2i and receives i2r.
        for (pt, ct) in v.steps("i2r_before_rekey") {
            assert_eq!(t.decrypt(&ct).unwrap(), pt);
        }
        for (pt, ct) in v.steps("r2i_before_rekey") {
            assert_eq!(t.encrypt(&pt).unwrap(), ct);
        }
        assert_eq!(t.sent_count(), 3);
        t.rekey_incoming();
        t.rekey_outgoing();
        assert_eq!(t.sent_count(), 0);
        for (pt, ct) in v.steps("i2r_after_rekey") {
            assert_eq!(t.decrypt(&ct).unwrap(), pt);
        }
        for (pt, ct) in v.steps("r2i_after_rekey") {
            assert_eq!(t.encrypt(&pt).unwrap(), ct);
        }
    }

    #[test]
    fn initiator_reproduces_ts_bytes_through_rekey() {
        let v = V::load();
        let (desktop, device, channel) = v.ids();
        let mut i = Initiator::with_ephemeral(
            &arr(v.input("initiator_static_private")),
            &arr(v.input("responder_static_public")),
            &desktop,
            &device,
            channel,
            &arr(v.input("initiator_ephemeral_private")),
        )
        .unwrap();
        let hs1 = i.write_hs1(&unhex(v.input("hs1_payload"))).unwrap();
        assert_eq!(hs1, unhex(v.output("hs1")));
        let (payload, mut t) = i.read_hs2(&unhex(v.output("hs2"))).unwrap();
        assert_eq!(payload, unhex(v.input("hs2_payload")));
        for (pt, ct) in v.steps("i2r_before_rekey") {
            assert_eq!(t.encrypt(&pt).unwrap(), ct);
        }
        for (pt, ct) in v.steps("r2i_before_rekey") {
            assert_eq!(t.decrypt(&ct).unwrap(), pt);
        }
        t.rekey_outgoing();
        t.rekey_incoming();
        for (pt, ct) in v.steps("i2r_after_rekey") {
            assert_eq!(t.encrypt(&pt).unwrap(), ct);
        }
        for (pt, ct) in v.steps("r2i_after_rekey") {
            assert_eq!(t.decrypt(&ct).unwrap(), pt);
        }
    }

    #[test]
    fn wrong_prologue_fails() {
        let v = V::load();
        let (desktop, device, channel) = v.ids();
        let bad = &v.root["wrong_prologue"];
        assert_ne!(
            unhex(bad["prologue"].as_str().unwrap()),
            unhex(v.input("prologue"))
        );
        let mut r = Responder::new(
            &arr(v.input("responder_static_private")),
            &desktop,
            &device,
            channel,
        )
        .unwrap();
        assert!(r.read_hs1(&unhex(bad["hs1"].as_str().unwrap())).is_err());
    }

    #[test]
    fn wrong_responder_static_fails() {
        let v = V::load();
        let (desktop, device, channel) = v.ids();
        let bad = &v.root["wrong_responder_static"];
        let mut r = Responder::new(
            &arr(v.input("responder_static_private")),
            &desktop,
            &device,
            channel,
        )
        .unwrap();
        assert!(r.read_hs1(&unhex(bad["hs1"].as_str().unwrap())).is_err());
    }

    #[test]
    fn random_roundtrip_and_limits() {
        let (rs_priv, rs_pub) = generate_static().unwrap();
        let (is_priv, _) = generate_static().unwrap();
        let (desktop, device) = ([1u8; 16], [2u8; 16]);
        let mut r = Responder::new(&rs_priv, &desktop, &device, 3).unwrap();
        let mut i = Initiator::new(&is_priv, &rs_pub, &desktop, &device, 3).unwrap();
        let hs1 = i.write_hs1(b"hello").unwrap();
        let (p, _) = r.read_hs1(&hs1).unwrap();
        assert_eq!(p, b"hello");
        let (hs2, mut rt) = r.write_hs2(b"world").unwrap();
        let (p, mut it) = i.read_hs2(&hs2).unwrap();
        assert_eq!(p, b"world");
        let big = vec![7u8; MAX_NOISE_PLAINTEXT];
        let ct = it.encrypt(&big).unwrap();
        assert_eq!(ct.len(), MAX_NOISE_MESSAGE);
        assert_eq!(rt.decrypt(&ct).unwrap(), big);
        assert_eq!(
            it.encrypt(&vec![0u8; MAX_NOISE_PLAINTEXT + 1]),
            Err(NoiseError::TooLarge)
        );
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(rt.decrypt(&bad), Err(NoiseError::Decrypt));
    }

    #[test]
    fn known_small_order_points_are_low_order_in_every_encoding() {
        let hex = |h: &str| -> [u8; 32] { unhex(h).try_into().unwrap() };
        let known = [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0100000000000000000000000000000000000000000000000000000000000000",
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ];
        for point in known {
            let key = hex(point);
            assert!(is_low_order(&key), "{point}");
            // The top bit is ignored by X25519, so the high-bit twin is too.
            let mut twin = key;
            twin[31] |= 0x80;
            assert!(is_low_order(&twin), "{point} with the top bit set");
        }
        // Real keys are not low order.
        for _ in 0..8 {
            let (_, public) = generate_static().unwrap();
            assert!(!is_low_order(&public));
        }
        let mut near = hex(known[1]);
        near[0] = 2;
        assert!(!is_low_order(&near));
    }
}
