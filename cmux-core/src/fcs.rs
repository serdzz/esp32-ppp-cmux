//! CRC-8 frame check sequence per 3GPP TS 27.010 §5.2.1.6.
//!
//! Polynomial x^8 + x^2 + x + 1 (0x07), reflected (0xE0), initial value 0xFF.
//! The transmitted FCS is the bitwise NOT of the running CRC (`0xFF - crc`).

const INIT: u8 = 0xFF;

/// Reflected CRC-8 lookup table for poly 0xE0.
///
/// Generated once and committed to avoid build.rs complexity. Verified against
/// the table in Annex B of TS 27.010 and the Linux kernel `n_gsm.c`.
const TABLE: [u8; 256] = build_table();

const fn build_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0u16;
    while i < 256 {
        let mut crc = i as u8;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 0x01 != 0 {
                (crc >> 1) ^ 0xE0
            } else {
                crc >> 1
            };
            j += 1;
        }
        t[i as usize] = crc;
        i += 1;
    }
    t
}

/// Update the running CRC with one byte.
#[inline]
pub fn update(crc: u8, byte: u8) -> u8 {
    TABLE[(crc ^ byte) as usize]
}

/// Compute the CRC across a slice (without inverting yet).
pub fn run(bytes: &[u8]) -> u8 {
    let mut crc = INIT;
    for &b in bytes {
        crc = update(crc, b);
    }
    crc
}

/// Compute the on-wire FCS (`0xFF - crc`) for the supplied header bytes.
///
/// Caller passes Address + Control + Length octet(s). Per spec, info bytes
/// are *not* covered by FCS for SABM/UA/DM/DISC/UIH frames.
pub fn fcs(header: &[u8]) -> u8 {
    0xFF - run(header)
}

/// Validate a received FCS against the header bytes used to compute it.
pub fn check(header: &[u8], received_fcs: u8) -> bool {
    // After running over header + received_fcs, a good frame yields 0xCF.
    // (See spec Annex B; 0xCF is the "good FCS" magic for the reflected poly.)
    let mut crc = run(header);
    crc = update(crc, received_fcs);
    crc == 0xCF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_random() {
        for hdr in [
            &[0x03u8, 0x3F, 0x01][..],
            &[0x07, 0xEF, 0x09],
            &[0x0B, 0x73, 0x01],
        ] {
            let f = fcs(hdr);
            assert!(check(hdr, f), "header {:?} fcs {:#x}", hdr, f);
        }
    }

    #[test]
    fn known_vector_sabm_dlci0() {
        // Address=0x03 (DLCI=0, C/R=1, EA=1), Control=SABM|PF=0x3F, Length=0x01 (len=0).
        // Reference value from Linux n_gsm test vectors.
        assert_eq!(fcs(&[0x03, 0x3F, 0x01]), 0x1C);
    }

    #[test]
    fn rejects_corrupted() {
        let hdr = &[0x03u8, 0x3F, 0x01];
        let f = fcs(hdr);
        assert!(!check(hdr, f ^ 0x01));
    }
}
