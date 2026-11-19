//! jpvoice: AES-128 (encryption only, which is all CTR mode needs), for
//! NCCH partitions encrypted with the fixed key. An application's fixed
//! key is the all-zero key, a public constant: no console key is involved.
//! Checked against FIPS-197's example vector and the zero-key vector.

const SBOX: [u8; 256] = {
    // computed at compile time from the field inverse and the affine map
    let mut sbox = [0u8; 256];
    let mut p: u8 = 1;
    let mut q: u8 = 1;
    loop {
        // p * 3
        p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1B } else { 0 };
        // q / 3
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        sbox[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    sbox[0] = 0x63;
    sbox
};

fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1B } else { 0 }
}

pub struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Aes128 {
        let mut w = [[0u8; 4]; 44];
        for i in 0..4 {
            w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        let mut rcon = 1u8;
        for i in 4..44 {
            let mut t = w[i - 1];
            if i % 4 == 0 {
                t = [SBOX[t[1] as usize] ^ rcon, SBOX[t[2] as usize], SBOX[t[3] as usize], SBOX[t[0] as usize]];
                rcon = xtime(rcon);
            }
            for k in 0..4 {
                w[i][k] = w[i - 4][k] ^ t[k];
            }
        }
        let mut round_keys = [[0u8; 16]; 11];
        for (r, key) in round_keys.iter_mut().enumerate() {
            for c in 0..4 {
                key[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
        }
        Aes128 { round_keys }
    }

    pub fn encrypt(&self, block: &mut [u8; 16]) {
        let add = |s: &mut [u8; 16], k: &[u8; 16]| s.iter_mut().zip(k).for_each(|(a, b)| *a ^= b);
        add(block, &self.round_keys[0]);
        for round in 1..11 {
            // SubBytes and ShiftRows (column-major state: byte r + 4c)
            let s = *block;
            for c in 0..4 {
                for r in 0..4 {
                    block[r + 4 * c] = SBOX[s[r + 4 * ((c + r) % 4)] as usize];
                }
            }
            if round != 10 {
                for c in 0..4 {
                    let col = [block[4 * c], block[4 * c + 1], block[4 * c + 2], block[4 * c + 3]];
                    let all = col[0] ^ col[1] ^ col[2] ^ col[3];
                    for r in 0..4 {
                        block[4 * c + r] = col[r] ^ all ^ xtime(col[r] ^ col[(r + 1) % 4]);
                    }
                }
            }
            add(block, &self.round_keys[round]);
        }
    }
}

/// AES-CTR with a 128-bit big-endian counter: XORs data in place, data
/// starting at block number `block` after the counter `base`.
pub fn ctr_xor(aes: &Aes128, base: &[u8; 16], block: u64, data: &mut [u8]) {
    let start = u128::from_be_bytes(*base).wrapping_add(block as u128);
    for (i, chunk) in data.chunks_mut(16).enumerate() {
        let mut keystream = start.wrapping_add(i as u128).to_be_bytes();
        aes.encrypt(&mut keystream);
        chunk.iter_mut().zip(keystream).for_each(|(b, k)| *b ^= k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> [u8; 16] {
        let mut out = [0u8; 16];
        for i in 0..16 {
            out[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn fips197_vector() {
        let aes = Aes128::new(&hex("000102030405060708090a0b0c0d0e0f"));
        let mut block = hex("00112233445566778899aabbccddeeff");
        aes.encrypt(&mut block);
        assert_eq!(block, hex("69c4e0d86a7b0430d8cdb78070b4c55a"));
    }

    #[test]
    fn zero_key_vector() {
        let aes = Aes128::new(&[0; 16]);
        let mut block = [0u8; 16];
        aes.encrypt(&mut block);
        assert_eq!(block, hex("66e94bd4ef8a2c3b884cfa59ca342b2e"));
    }

    #[test]
    fn ctr_round_trip_and_offsets() {
        let aes = Aes128::new(&[0; 16]);
        let base = hex("0004000000179800030000000000ffff");
        let plain: Vec<u8> = (0..100u8).collect();
        let mut data = plain.clone();
        ctr_xor(&aes, &base, 0, &mut data);
        assert_ne!(data, plain);
        // decrypting from the second block on gives the same bytes there
        let mut tail = data[16..].to_vec();
        ctr_xor(&aes, &base, 1, &mut tail);
        assert_eq!(&tail[..], &plain[16..]);
        ctr_xor(&aes, &base, 0, &mut data);
        assert_eq!(data, plain);
    }
}
