//! The engine's case-insensitive name hash, used to match animation channels to skeleton bones
//! (and for clip names, channel names and named data blocks).
//!
//! `h = h * 0x397 + rank(c)` over the bytes of the name, where `rank` is a fixed ordering of the
//! printable ASCII characters in which upper and lower case letters share a rank.

/// Rank of the printable ASCII characters `' '..='~'`.
#[rustfmt::skip]
const RANK: [u8; 95] = [
    // ' '  !   "   #   $   %   &   '   (   )   *   +   ,   -   .   /
       37, 58, 51, 60, 61, 62, 64, 50, 66, 67, 65, 40, 54, 39, 55, 52,
    // 0..9
       27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
    // :   ;   <   =   >   ?   @
       49, 48, 56, 41, 57, 53, 59,
    // A..Z
        1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13,
       14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    // [   \   ]   ^   _   `
       44, 42, 45, 63, 38, 68,
    // a..z
        1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13,
       14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    // {   |   }   ~
       46, 43, 47, 69,
];

fn rank(byte: u8) -> u32 {
    match byte {
        b' '..=b'~' => RANK[(byte - b' ') as usize] as u32,
        _ => 0,
    }
}

pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(0u32, |h, b| h.wrapping_mul(0x397).wrapping_add(rank(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive() {
        assert_eq!(name_hash("Pelvis"), name_hash("pElViS"));
    }

    #[test]
    fn known_values() {
        // Observed in retail animation banks.
        assert_eq!(name_hash("Rotation"), 0x2743_6396);
        assert_eq!(name_hash("Pos"), 0x00ce_66fc);
        assert_eq!(name_hash("LITH"), 0x2b99_dbc1);
    }
}
