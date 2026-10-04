//! LSP position encoding adapters.
//!
//! LSP `Position.character` is an offset in the negotiated encoding's code
//! units. These helpers translate between a buffer byte offset and that
//! offset for one line of text, rejecting anything that cannot map cleanly:
//! non-boundary bytes, out-of-range characters, and UTF-16 units that would
//! land inside a surrogate pair. Returning `None` — never clamping or slicing
//! mid-char — is what keeps a malformed server range from corrupting UTF-8.

/// Position encodings an LSP server may negotiate (`positionEncoding`).
///
/// Staged surface: consumed by the Task 2 transport and the Task 3+ client;
/// only unit tests touch it until then.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    /// UTF-8 code units: one unit per byte.
    Utf8,
    /// UTF-16 code units: the protocol fallback when a server sends no
    /// `positionEncoding` or an unrecognized value.
    #[default]
    Utf16,
    /// UTF-32 code units: one unit per Unicode scalar value.
    Utf32,
}

#[allow(dead_code)]
impl PositionEncoding {
    /// Resolve the server's `capabilities.positionEncoding` value.
    ///
    /// Per the protocol the value may be absent or a value we do not support;
    /// both fall back to UTF-16, the protocol default.
    pub fn from_capability(server_choice: Option<&str>) -> Self {
        match server_choice {
            Some("utf-8") => Self::Utf8,
            Some("utf-32") => Self::Utf32,
            _ => Self::Utf16,
        }
    }

    /// The wire string this encoding reports in `positionEncoding` fields.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }

    /// Code units contributed by one scalar value.
    fn units_of(&self, c: char) -> usize {
        match self {
            Self::Utf8 => c.len_utf8(),
            Self::Utf16 => c.len_utf16(),
            Self::Utf32 => 1,
        }
    }
}

/// Convert a byte offset within `text` to an LSP character offset.
///
/// Returns `None` when `byte` is out of range or not a char boundary — a
/// byte inside a multi-byte scalar has no well-defined character offset.
/// Staged for Task 3+ document sync; only tests consume it today.
#[allow(dead_code)]
pub fn byte_to_lsp(text: &str, byte: usize, encoding: PositionEncoding) -> Option<usize> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    Some(text[..byte].chars().map(|c| encoding.units_of(c)).sum())
}

/// Convert an LSP character offset within `text` to a byte offset.
///
/// Returns `None` when `character` overshoots the text or would land inside
/// a scalar's unit sequence — e.g. a UTF-16 offset in the middle of a
/// surrogate pair (a non-BMP character counts 2 units and offsets 1 unit in
/// are rejected). Staged for Task 3+ ranges; only tests consume it today.
#[allow(dead_code)]
pub fn lsp_to_byte(text: &str, character: usize, encoding: PositionEncoding) -> Option<usize> {
    let mut units = 0usize;
    let mut byte = 0usize;
    for c in text.chars() {
        if units == character {
            return Some(byte);
        }
        units += encoding.units_of(c);
        byte += c.len_utf8();
        if units > character {
            // `character` sits inside this scalar's unit sequence.
            return None;
        }
    }
    (units == character).then_some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [PositionEncoding; 3] = [
        PositionEncoding::Utf8,
        PositionEncoding::Utf16,
        PositionEncoding::Utf32,
    ];

    #[test]
    fn plan_matrix_byte_to_lsp_and_back() {
        // "a😀b": bytes 1+4+1; UTF-16 units 1+2+1.
        assert_eq!(byte_to_lsp("a😀b", 5, PositionEncoding::Utf16).unwrap(), 3);
        assert_eq!(lsp_to_byte("a😀b", 3, PositionEncoding::Utf16).unwrap(), 5);
    }

    #[test]
    fn round_trips_every_boundary_in_all_encodings() {
        for text in [
            "ascii",
            "中文行",
            "a😀b\u{1f980}c",
            "e\u{301}combining",
            "tab\there",
            "trailing\r",
            "edge\ncase", // callers pass single lines; \n is just a scalar here
        ] {
            for enc in ALL {
                for (byte, _) in text.char_indices().chain([(text.len(), ' ')]) {
                    let ch = byte_to_lsp(text, byte, enc).unwrap();
                    assert_eq!(
                        lsp_to_byte(text, ch, enc),
                        Some(byte),
                        "round trip {text:?} byte {byte} {enc:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn utf8_offsets_are_byte_offsets_but_still_boundary_checked() {
        assert_eq!(byte_to_lsp("ab", 1, PositionEncoding::Utf8).unwrap(), 1);
        assert_eq!(lsp_to_byte("ab", 2, PositionEncoding::Utf8).unwrap(), 2);
        // A byte offset inside a multi-byte scalar is rejected in UTF-8 too.
        assert_eq!(byte_to_lsp("😀", 2, PositionEncoding::Utf8), None);
        assert_eq!(lsp_to_byte("😀", 2, PositionEncoding::Utf8), None);
        assert_eq!(lsp_to_byte("😀", 4, PositionEncoding::Utf8).unwrap(), 4);
    }

    #[test]
    fn utf16_rejects_surrogate_interior_offsets() {
        // 😀 = 2 UTF-16 units at boundary 1 in "a😀b"; interior offset 2 must
        // fail rather than slice the surrogate pair.
        assert_eq!(lsp_to_byte("a😀b", 2, PositionEncoding::Utf16), None);
        assert_eq!(lsp_to_byte("a😀b", 0, PositionEncoding::Utf16).unwrap(), 0);
        assert_eq!(lsp_to_byte("a😀b", 4, PositionEncoding::Utf16).unwrap(), 6);
        // One unit past the end is out of range.
        assert_eq!(lsp_to_byte("a😀b", 5, PositionEncoding::Utf16), None);
    }

    #[test]
    fn utf32_counts_scalar_values() {
        assert_eq!(byte_to_lsp("a😀b", 5, PositionEncoding::Utf32).unwrap(), 2);
        assert_eq!(lsp_to_byte("a😀b", 2, PositionEncoding::Utf32).unwrap(), 5);
        assert_eq!(lsp_to_byte("a😀b", 3, PositionEncoding::Utf32).unwrap(), 6);
        assert_eq!(lsp_to_byte("a😀b", 4, PositionEncoding::Utf32), None);
    }

    #[test]
    fn invalid_ranges_and_boundaries_are_rejected() {
        for enc in ALL {
            assert_eq!(byte_to_lsp("ab", 3, enc), None);
            assert_eq!(byte_to_lsp("😀", 2, enc), None);
            assert_eq!(lsp_to_byte("ab", 3, enc), None);
        }
        // Empty text only maps position 0.
        assert_eq!(byte_to_lsp("", 0, PositionEncoding::Utf16).unwrap(), 0);
        assert_eq!(lsp_to_byte("", 0, PositionEncoding::Utf16).unwrap(), 0);
        assert_eq!(lsp_to_byte("", 1, PositionEncoding::Utf16), None);
    }

    #[test]
    fn crlf_and_combining_count_as_scalars_not_graphemes() {
        // "\r" is one scalar/unit on a line that keeps its CR.
        assert_eq!(byte_to_lsp("x\r", 2, PositionEncoding::Utf16).unwrap(), 2);
        // e + combining acute = 2 scalars: 3 UTF-8 units (U+0301 is 2 bytes),
        // 2 UTF-16 and UTF-32 units.
        let e_acute = "e\u{301}";
        assert_eq!(
            byte_to_lsp(e_acute, e_acute.len(), PositionEncoding::Utf8).unwrap(),
            3
        );
        assert_eq!(
            byte_to_lsp(e_acute, e_acute.len(), PositionEncoding::Utf16).unwrap(),
            2
        );
        assert_eq!(
            byte_to_lsp(e_acute, e_acute.len(), PositionEncoding::Utf32).unwrap(),
            2
        );
    }

    #[test]
    fn negotiation_uses_utf16_only_as_protocol_fallback() {
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-8")),
            PositionEncoding::Utf8
        );
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-32")),
            PositionEncoding::Utf32
        );
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-16")),
            PositionEncoding::Utf16
        );
        // Absent or unrecognized → the protocol fallback, UTF-16.
        assert_eq!(
            PositionEncoding::from_capability(None),
            PositionEncoding::Utf16
        );
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-64")),
            PositionEncoding::Utf16
        );
        for enc in ALL {
            assert_eq!(PositionEncoding::from_capability(Some(enc.as_str())), enc);
        }
    }
}
