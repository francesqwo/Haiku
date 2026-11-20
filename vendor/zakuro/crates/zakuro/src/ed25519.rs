//! updater: Ed25519 signature checking (RFC 8032, PureEdDSA), for Haiku's
//! updates: a release's Haiku.zip is installed only when Haiku.zip.sig, the
//! zip's signature by Haiku's release key, checks out against the public key
//! built into Haiku.exe (haiku_update.rs).
//!
//! Why this file: no Ed25519 crate (ed25519-dalek, curve25519-dalek, ring)
//! is in Cargo.lock or in the offline cargo cache this project builds from
//! (`--locked --offline`), so none could be added. This is a port of
//! TweetNaCl's verification (`crypto_sign_open`: Bernstein, van Gastel,
//! Janssen, Lange, Schwabe, Smetsers; public domain), the smallest widely
//! reviewed Ed25519, line for line: the field arithmetic over 16 limbs of
//! 16 bits, the point addition, the double-and-add, the reduction mod L. Its
//! SHA-512 is the sha2 crate's, already in Haiku. Two things are added to
//! it, as RFC 8032 section 5.1.7 asks: S must be below L (no malleable
//! signatures), and the message is hashed as it is read, so a 50 MB zip is
//! never held twice. Only public data goes through it (the key, the
//! signature, the zip), so it need not be constant time; it is anyway, as
//! TweetNaCl is.
//!
//! Checked against RFC 8032's test vectors and against OpenSSL's signatures
//! (the tests below; dev/release/sign.sh signs with OpenSSL).

use sha2::{Digest, Sha512};

type Gf = [i64; 16];

const GF0: Gf = [0; 16];
const GF1: Gf = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const D: Gf = [0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070, 0xe898, 0x7779, 0x4079, 0x8cc7, 0xfe73, 0x2b6f, 0x6cee, 0x5203];
const D2: Gf = [0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0, 0xd130, 0xeef3, 0x80f2, 0x198e, 0xfce7, 0x56df, 0xd9dc, 0x2406];
const X: Gf = [0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c, 0xdc5c, 0xfdd6, 0xe231, 0xc0a4, 0x53fe, 0xcd6e, 0x36d3, 0x2169];
const Y: Gf = [0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666];
const I: Gf = [0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43, 0xd7a7, 0x3dfb, 0x0099, 0x2b4d, 0xdf0b, 0x4fc1, 0x2480, 0x2b83];
/// the group's order, L = 2^252 + 27742317777372353535851937790883648493,
/// little-endian bytes
const L: [i64; 32] = [0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10];

fn car25519(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

fn sel25519(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn pack25519(n: &Gf) -> [u8; 32] {
    let mut t = *n;
    car25519(&mut t);
    car25519(&mut t);
    car25519(&mut t);
    let mut m = GF0;
    for _ in 0..2 {
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    let mut o = [0u8; 32];
    for i in 0..16 {
        o[2 * i] = (t[i] & 0xff) as u8;
        o[2 * i + 1] = (t[i] >> 8) as u8;
    }
    o
}

fn neq25519(a: &Gf, b: &Gf) -> bool {
    pack25519(a) != pack25519(b)
}

fn par25519(a: &Gf) -> u8 {
    pack25519(a)[0] & 1
}

fn unpack25519(n: &[u8; 32]) -> Gf {
    let mut o = GF0;
    for i in 0..16 {
        o[i] = n[2 * i] as i64 + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

fn add_gf(a: &Gf, b: &Gf) -> Gf {
    let mut o = GF0;
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
    o
}

fn sub_gf(a: &Gf, b: &Gf) -> Gf {
    let mut o = GF0;
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
    o
}

fn mul_gf(a: &Gf, b: &Gf) -> Gf {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o = GF0;
    o.copy_from_slice(&t[..16]);
    car25519(&mut o);
    car25519(&mut o);
    o
}

fn sq_gf(a: &Gf) -> Gf {
    mul_gf(a, a)
}

fn inv25519(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = sq_gf(&c);
        if a != 2 && a != 4 {
            c = mul_gf(&c, i);
        }
    }
    c
}

fn pow2523(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=250).rev() {
        c = sq_gf(&c);
        if a != 1 {
            c = mul_gf(&c, i);
        }
    }
    c
}

/// a point in extended coordinates (X, Y, Z, T)
type Point = [Gf; 4];

fn add(p: &mut Point, q: &Point) {
    let a = mul_gf(&sub_gf(&p[1], &p[0]), &sub_gf(&q[1], &q[0]));
    let b = mul_gf(&add_gf(&p[0], &p[1]), &add_gf(&q[0], &q[1]));
    let c = mul_gf(&mul_gf(&p[3], &q[3]), &D2);
    let d = mul_gf(&p[2], &q[2]);
    let d = add_gf(&d, &d);
    let e = sub_gf(&b, &a);
    let f = sub_gf(&d, &c);
    let g = add_gf(&d, &c);
    let h = add_gf(&b, &a);
    p[0] = mul_gf(&e, &f);
    p[1] = mul_gf(&h, &g);
    p[2] = mul_gf(&g, &f);
    p[3] = mul_gf(&e, &h);
}

fn cswap(p: &mut Point, q: &mut Point, b: u8) {
    for i in 0..4 {
        sel25519(&mut p[i], &mut q[i], b as i64);
    }
}

fn pack(p: &Point) -> [u8; 32] {
    let zi = inv25519(&p[2]);
    let tx = mul_gf(&p[0], &zi);
    let ty = mul_gf(&p[1], &zi);
    let mut r = pack25519(&ty);
    r[31] ^= par25519(&tx) << 7;
    r
}

fn scalarmult(q: &mut Point, s: &[u8; 32]) -> Point {
    let mut p: Point = [GF0, GF1, GF1, GF0];
    for i in (0..256).rev() {
        let b = (s[i / 8] >> (i & 7)) & 1;
        cswap(&mut p, q, b);
        let pp = p;
        add(q, &pp);
        add(&mut p, &pp);
        cswap(&mut p, q, b);
    }
    p
}

fn scalarbase(s: &[u8; 32]) -> Point {
    let mut q: Point = [X, Y, GF1, mul_gf(&X, &Y)];
    scalarmult(&mut q, s)
}

fn mod_l(x: &mut [i64; 64]) -> [u8; 32] {
    for i in (32..64).rev() {
        let mut carry = 0;
        let mut j = i - 32;
        while j < i - 12 {
            x[j] += carry - 16 * x[i] * L[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry << 8;
            j += 1;
        }
        x[j] += carry;
        x[i] = 0;
    }
    let mut carry = 0;
    for j in 0..32 {
        x[j] += carry - (x[31] >> 4) * L[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for j in 0..32 {
        x[j] -= carry * L[j];
    }
    let mut r = [0u8; 32];
    for i in 0..32 {
        x[i + 1] += x[i] >> 8;
        r[i] = (x[i] & 255) as u8;
    }
    r
}

fn reduce(h: &[u8; 64]) -> [u8; 32] {
    let mut x = [0i64; 64];
    for i in 0..64 {
        x[i] = h[i] as i64;
    }
    mod_l(&mut x)
}

/// -A from the public key, none when it isn't a point of the curve.
fn unpackneg(p: &[u8; 32]) -> Option<Point> {
    let mut r: Point = [GF0, GF0, GF1, GF0];
    r[1] = unpack25519(p);
    let num = sq_gf(&r[1]);
    let den = mul_gf(&num, &D);
    let num = sub_gf(&num, &r[2]);
    let den = add_gf(&r[2], &den);
    let den2 = sq_gf(&den);
    let den4 = sq_gf(&den2);
    let den6 = mul_gf(&den4, &den2);
    let mut t = mul_gf(&den6, &num);
    t = mul_gf(&t, &den);
    t = pow2523(&t);
    t = mul_gf(&t, &num);
    t = mul_gf(&t, &den);
    t = mul_gf(&t, &den);
    r[0] = mul_gf(&t, &den);
    let chk = mul_gf(&sq_gf(&r[0]), &den);
    if neq25519(&chk, &num) {
        r[0] = mul_gf(&r[0], &I);
    }
    let chk = mul_gf(&sq_gf(&r[0]), &den);
    if neq25519(&chk, &num) {
        return None;
    }
    if par25519(&r[0]) == (p[31] >> 7) {
        r[0] = sub_gf(&GF0, &r[0]);
    }
    r[3] = mul_gf(&r[0], &r[1]);
    Some(r)
}

/// whether the 32 little-endian bytes are a number below L.
fn below_l(s: &[u8]) -> bool {
    for i in (0..32).rev() {
        let (a, b) = (s[i] as i64, L[i]);
        if a != b {
            return a < b;
        }
    }
    false
}

/// whether signature is public's signature of the message read from
/// message (RFC 8032's Ed25519, the message not pre-hashed). A read error
/// is an error, a bad signature Ok(false).
pub fn verify_reader(public: &[u8; 32], signature: &[u8; 64], message: &mut dyn std::io::Read) -> std::io::Result<bool> {
    let s: [u8; 32] = signature[32..].try_into().expect("32 bytes");
    if !below_l(&s) {
        return Ok(false);
    }
    let Some(mut a) = unpackneg(public) else { return Ok(false) };
    let mut hash = Sha512::new();
    hash.update(&signature[..32]);
    hash.update(public);
    std::io::copy(message, &mut hash)?;
    let h: [u8; 64] = hash.finalize().into();
    let h = reduce(&h);
    let mut p = scalarmult(&mut a, &h);
    let q = scalarbase(&s);
    add(&mut p, &q);
    Ok(pack(&p) == signature[..32])
}

/// whether signature is public's signature of message.
#[cfg_attr(not(test), allow(dead_code))]
pub fn verify(public: &[u8; 32], signature: &[u8; 64], message: &[u8]) -> bool {
    verify_reader(public, signature, &mut &message[..]).unwrap_or(false)
}

/// bytes from hex, none when it isn't hex of that length.
pub fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h<const N: usize>(text: &str) -> [u8; N] {
        hex(text).expect("hex")
    }

    /// RFC 8032 section 7.1, tests 1, 2, 3 and SHA(abc)
    #[test]
    fn the_rfc_8032_test_vectors_verify() {
        let cases: [(&str, &str, &str); 4] = [
            (
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "",
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
            ),
            (
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "72",
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
            ),
            (
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "af82",
                "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
            ),
            (
                "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
                "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b58909351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
            ),
        ];
        for (public, message, signature) in cases {
            let public: [u8; 32] = h(public);
            let signature: [u8; 64] = h(signature);
            let message: Vec<u8> = (0..message.len() / 2).map(|i| u8::from_str_radix(&message[i * 2..i * 2 + 2], 16).unwrap()).collect();
            assert!(verify(&public, &signature, &message), "RFC 8032 vector {}", hex_of(&public));
            // one bit off anywhere, and it fails
            let mut bad = message.clone();
            bad.push(0);
            assert!(!verify(&public, &signature, &bad));
            let mut bad_sig = signature;
            bad_sig[5] ^= 0x10;
            assert!(!verify(&public, &bad_sig, &message));
            let mut bad_sig = signature;
            bad_sig[40] ^= 0x01;
            assert!(!verify(&public, &bad_sig, &message));
            let mut other = public;
            other[3] ^= 0x04;
            assert!(!verify(&other, &signature, &message));
        }
    }

    /// S + L is the same point but a malleated signature: refused.
    #[test]
    fn a_signature_with_s_past_l_is_refused() {
        let public: [u8; 32] = h("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let mut signature: [u8; 64] = h("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b");
        let mut carry = 0u16;
        for i in 0..32 {
            let sum = signature[32 + i] as u16 + L[i] as u16 + carry;
            signature[32 + i] = sum as u8;
            carry = sum >> 8;
        }
        assert!(!verify(&public, &signature, b""));
    }

    /// a signature OpenSSL made (dev/release/sign.sh's way: `openssl
    /// pkeyutl -sign -rawin`) of a longer message, read in pieces.
    #[test]
    fn an_openssl_signature_of_a_longer_message_verifies_as_it_is_read() {
        // the key of RFC 8032's test 1, the message 100,000 bytes of i*7
        let public: [u8; 32] = h("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let message: Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(7)) as u8).collect();
        let signature: [u8; 64] = h(OPENSSL_LONG);
        assert!(verify(&public, &signature, &message));
        let mut reader = std::io::BufReader::with_capacity(4096, &message[..]);
        assert!(verify_reader(&public, &signature, &mut reader).unwrap());
        let mut changed = message.clone();
        changed[77_777] ^= 1;
        assert!(!verify(&public, &signature, &changed));
    }

    /// `openssl pkeyutl -sign -rawin` with RFC 8032 test 1's secret key over
    /// the 100,000 bytes above (its DER: 302e020100300506032b657004220420
    /// and the 32-byte seed, through `openssl pkey -inform DER`)
    const OPENSSL_LONG: &str = "96dd204f331007174a5b2390c9444c42039bb31884c26b671886be7a79b1a5de3d6aef72848dd90bccd893f341a4faba79538135a2dbacf40767e779517f4206";

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn hex_is_read_or_refused() {
        assert_eq!(hex::<2>("0aFf"), Some([0x0a, 0xff]));
        assert_eq!(hex::<2>("0aF"), None);
        assert_eq!(hex::<2>("zz00"), None);
    }
}
