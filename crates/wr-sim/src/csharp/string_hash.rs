//! `System.String.GetHashCode()` of the game's runtime (the shipped Unity 2020.3 Mono
//! `mscorlib.dll`, decompiled with `ilspycmd` into the scratchpad, not kept): the legacy,
//! non-randomized hash over the UTF-16 code units, two interleaved djb2-style lanes, stopping at
//! the first NUL character (the loop reads the null-terminated buffer).

/// `string.GetHashCode()`.
#[must_use]
pub fn get_hash_code(s: &str) -> i32 {
    let units: Vec<u16> = s.encode_utf16().collect();
    let at = |i: usize| -> i32 { units.get(i).copied().map_or(0, i32::from) };
    let mut h1: i32 = 5381;
    let mut h2: i32 = h1;
    let mut i = 0;
    loop {
        let c = at(i);
        if c == 0 {
            break;
        }
        h1 = (h1 << 5).wrapping_add(h1) ^ c;
        let c = at(i + 1);
        if c == 0 {
            break;
        }
        h2 = (h2 << 5).wrapping_add(h2) ^ c;
        i += 2;
    }
    h1.wrapping_add(h2.wrapping_mul(1_566_083_941))
}

#[cfg(test)]
mod tests {
    use super::get_hash_code;

    /// Values worked by hand from the decompiled loop.
    #[test]
    fn hashes_follow_the_mono_loop() {
        // "": both lanes stay 5381; 5381 + 5381 * 1566083941 (wrapping).
        assert_eq!(get_hash_code(""), 5381i32.wrapping_add(5381i32.wrapping_mul(1_566_083_941)));
        // "a": lane one takes 'a'; lane two stops before it starts.
        let h1 = (5381i32 << 5).wrapping_add(5381) ^ 97;
        assert_eq!(get_hash_code("a"), h1.wrapping_add(5381i32.wrapping_mul(1_566_083_941)));
        // "ab": lane two takes 'b'.
        let h2 = (5381i32 << 5).wrapping_add(5381) ^ 98;
        assert_eq!(get_hash_code("ab"), h1.wrapping_add(h2.wrapping_mul(1_566_083_941)));
        // The loop stops at an embedded NUL.
        assert_eq!(get_hash_code("a\0zzz"), get_hash_code("a"));
    }

    /// The saved `InteractionSkillCheckPart.DCOverride`s of Ember's saves are `DC +
    /// (Math.Abs(UniqueId.GetHashCode()) % 5 - 2)` (`InteractionDCVariation` 2): five parts,
    /// five matches (`docs/poc4/dialogue.md`, round 5 review).
    #[test]
    fn saved_dc_overrides_match_the_hash() {
        for (id, dc, saved) in [
            ("6fb4fca8-4bb7-460f-bc69-9e81ddd159bb", 12, 14),
            ("6fdbec9e-a0ee-4f73-9042-e2ad802c053b", 12, 11),
            ("ee0845e7-81b9-4733-9e6e-5ab8bc8885fc", 12, 14),
            ("f4331a99-fe1e-492d-9d01-c3e1876412b1", 15, 17),
            ("f6d839b7-ef37-4ed1-a2ab-c98a5785ffcd", 12, 12),
        ] {
            let h = get_hash_code(id);
            assert_eq!(dc + (h.abs() % 5 - 2), saved, "{id}");
        }
    }
}
