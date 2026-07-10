//! DXF code page ($DWGCODEPAGE) to encoding mapping.
//!
//! Maps DXF code page names to `encoding_rs` encodings, following the same
//! mapping table used by ACadSharp's `CadUtils._dxfEncodingMap`.

use encoding_rs::Encoding;

/// Get the `encoding_rs` encoding for a DXF code page string.
///
/// Returns `None` if the encoding is UTF-8 (no transcoding needed) or the
/// code page string is not recognized.
///
/// # Rules
/// - If the DXF version is AC1021 (AutoCAD 2007+) or later, UTF-8 is always
///   used regardless of $DWGCODEPAGE — callers should not call this function.
/// - Otherwise, the code page string (case-insensitive) is looked up in the
///   mapping table.
pub fn encoding_from_code_page(code_page: &str) -> Option<&'static Encoding> {
    match code_page.to_ascii_lowercase().as_str() {
        // Asian encodings
        "gb2312" | "ansi_936" => Some(encoding_rs::GBK),
        "big5" | "ansi_950" => Some(encoding_rs::BIG5),
        "korean" | "ansi_949" | "johab" => Some(encoding_rs::EUC_KR),
        "ansi_932" => Some(encoding_rs::SHIFT_JIS),

        // DOS/OEM code pages
        "dos437" => Some(encoding_rs::IBM866), // closest available in encoding_rs
        "dos850" => Some(encoding_rs::WINDOWS_1252), // Western European
        "dos852" => Some(encoding_rs::WINDOWS_1250), // Central European
        "dos855" | "dos866" => Some(encoding_rs::IBM866), // Cyrillic
        "dos857" => Some(encoding_rs::WINDOWS_1254), // Turkish
        "dos860" => Some(encoding_rs::WINDOWS_1252), // Portuguese
        "dos861" => Some(encoding_rs::WINDOWS_1252), // Icelandic
        "dos863" => Some(encoding_rs::WINDOWS_1252), // Canadian-French
        "dos865" => Some(encoding_rs::WINDOWS_1252), // Nordic
        "dos869" => Some(encoding_rs::WINDOWS_1253), // Greek

        // Windows/ANSI code pages
        "ansi_874" => Some(encoding_rs::WINDOWS_874),
        "ansi_1250" => Some(encoding_rs::WINDOWS_1250),
        "ansi_1251" => Some(encoding_rs::WINDOWS_1251),
        "ansi_1252" => Some(encoding_rs::WINDOWS_1252),
        "ansi_1253" => Some(encoding_rs::WINDOWS_1253),
        "ansi_1254" => Some(encoding_rs::WINDOWS_1254),
        "ansi_1255" => Some(encoding_rs::WINDOWS_1255),
        "ansi_1256" => Some(encoding_rs::WINDOWS_1256),
        "ansi_1257" => Some(encoding_rs::WINDOWS_1257),
        "ansi_1258" => Some(encoding_rs::WINDOWS_1258),

        // ISO encodings
        "iso8859-1" | "iso_8859-1" => Some(encoding_rs::WINDOWS_1252),
        "iso8859-2" | "iso_8859-2" => Some(encoding_rs::ISO_8859_2),
        "iso8859-3" | "iso_8859-3" => Some(encoding_rs::ISO_8859_3),
        "iso8859-4" | "iso_8859-4" => Some(encoding_rs::ISO_8859_4),
        "iso8859-5" | "iso_8859-5" => Some(encoding_rs::ISO_8859_5),
        "iso8859-6" | "iso_8859-6" => Some(encoding_rs::ISO_8859_6),
        "iso8859-7" | "iso_8859-7" => Some(encoding_rs::ISO_8859_7),
        "iso8859-8" | "iso_8859-8" => Some(encoding_rs::ISO_8859_8),
        "iso8859-9" | "iso_8859-9" => Some(encoding_rs::WINDOWS_1254),
        "iso8859-10" | "iso_8859-10" => Some(encoding_rs::ISO_8859_10),
        "iso8859-13" | "iso_8859-13" => Some(encoding_rs::ISO_8859_13),
        "iso8859-14" | "iso_8859-14" => Some(encoding_rs::ISO_8859_14),
        "iso8859-15" | "iso_8859-15" => Some(encoding_rs::ISO_8859_15),

        // KOI8-R (Russian)
        "koi8-r" => Some(encoding_rs::KOI8_R),
        "koi8-u" => Some(encoding_rs::KOI8_U),

        // ASCII / UTF-8 / no fallback needed
        "ascii" | "utf-8" | "utf8" | "unicode" => None,

        // Default: Windows-1252 (most common DXF fallback)
        _ => Some(encoding_rs::WINDOWS_1252),
    }
}

/// Get the `encoding_rs` encoding for a DWG numeric code page.
///
/// DWG files store the code page either as a Windows code page number
/// (e.g. 936 for GBK) OR as an AutoCAD-internal index (small values like
/// 30-39 used by some Chinese/CJK AutoCAD versions).
///
/// Returns `None` for truly unrecognized/zero code pages — callers should
/// fall back to [`detect_encoding_from_bytes`] in that case.
pub fn encoding_from_dwg_code_page(code_page: u16) -> Option<&'static Encoding> {
    match code_page {
        // Standard Windows code page numbers (reliable)
        874 => Some(encoding_rs::WINDOWS_874),   // Thai
        932 => Some(encoding_rs::SHIFT_JIS),      // Japanese
        936 => Some(encoding_rs::GBK),            // Simplified Chinese
        949 => Some(encoding_rs::EUC_KR),         // Korean
        950 => Some(encoding_rs::BIG5),           // Traditional Chinese
        1250 => Some(encoding_rs::WINDOWS_1250),  // Central European
        1251 => Some(encoding_rs::WINDOWS_1251),  // Cyrillic
        1252 => Some(encoding_rs::WINDOWS_1252),  // Western European
        1253 => Some(encoding_rs::WINDOWS_1253),  // Greek
        1254 => Some(encoding_rs::WINDOWS_1254),  // Turkish
        1255 => Some(encoding_rs::WINDOWS_1255),  // Hebrew
        1256 => Some(encoding_rs::WINDOWS_1256),  // Arabic
        1257 => Some(encoding_rs::WINDOWS_1257),  // Baltic
        1258 => Some(encoding_rs::WINDOWS_1258),  // Vietnamese

        // Small index values (0-99) used by some R2004/AC1018 files are NOT
        // reliable standard Windows code pages. Fall through to auto-detect.
        0..=99 => None,

        _ => None,
    }
}

/// Auto-detect text encoding from raw byte samples.
///
/// Examines high-byte sequences in the data and scores candidate CJK
/// encodings using a pair-by-pair validation approach that is tolerant
/// of noise in binary (bitstream) data.
///
/// # Algorithm
/// 1. Scan for byte pairs where the lead byte ≥ 0x81.
/// 2. For each candidate encoding, validate each pair individually
///    and count how many decode to CJK characters.
/// 3. Return the encoding with the highest CJK score, or WINDOWS_1252.
pub fn detect_encoding_from_bytes(data: &[u8]) -> &'static Encoding {
    // Collect byte pairs with high lead bytes (potential multi-byte sequences)
    let limit = data.len().min(256 * 1024); // scan up to 256KB
    let mut pairs: Vec<[u8; 2]> = Vec::new();
    let mut i = 0;
    while i < limit && pairs.len() < 8192 {
        if data[i] >= 0x81 {
            if i + 1 < limit {
                pairs.push([data[i], data[i + 1]]);
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    if pairs.is_empty() {
        return encoding_rs::WINDOWS_1252;
    }

    // Score each candidate encoding by CJK character count.
    // Validate EACH pair independently to tolerate noise from binary data.
    let candidates: &[(&'static Encoding, u8, u8)] = &[
        // (encoding, min_trail_byte, max_trail_byte)
        (encoding_rs::GBK, 0x40, 0xFE),       // GBK: lead 0x81-0xFE, trail 0x40-0xFE (!= 0x7F)
        (encoding_rs::BIG5, 0x40, 0xFE),      // Big5: lead 0x81-0xFE, trail 0x40-0x7E + 0xA1-0xFE
        (encoding_rs::SHIFT_JIS, 0x40, 0xFC), // Shift_JIS: lead 0x81-0x9F/0xE0-0xEF
        (encoding_rs::EUC_KR, 0xA1, 0xFE),   // EUC-KR: lead 0xA1-0xFE, trail 0xA1-0xFE
    ];

    let mut best_encoding: &'static Encoding = encoding_rs::WINDOWS_1252;
    let mut best_score = 0usize;

    for &(enc, min_trail, max_trail) in candidates {
        let mut score = 0usize;
        for pair in &pairs {
            let lead = pair[0];
            let trail = pair[1];
            // Quick structural validation
            if trail < min_trail || trail > max_trail {
                continue;
            }
            if enc == encoding_rs::GBK && trail == 0x7F {
                continue;
            }
            if enc == encoding_rs::SHIFT_JIS {
                // Shift_JIS lead bytes: 0x81-0x9F, 0xE0-0xEF
                if !((0x81..=0x9F).contains(&lead) || (0xE0..=0xEF).contains(&lead)) {
                    continue;
                }
            }
            // Decode this single pair
            let (decoded, _, _) = enc.decode(&pair[..]);
            let ch = decoded.chars().next().unwrap_or('\0');
            let cp = ch as u32;
            // Count CJK characters
            if (0x4E00..=0x9FFF).contains(&cp)      // CJK Unified Ideographs
                || (0x3400..=0x4DBF).contains(&cp)  // CJK Extension A
                || (0x2E80..=0x33FF).contains(&cp)  // CJK Radicals/Symbols
                || (0xAC00..=0xD7AF).contains(&cp)  // Hangul
                || (0x3040..=0x30FF).contains(&cp)  // Hiragana/Katakana
                || (0xFF00..=0xFFEF).contains(&cp)  // Fullwidth forms
            {
                score += 1;
            }
        }
        if score > best_score {
            best_score = score;
            best_encoding = enc;
        }
    }

    best_encoding
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ansi_1252() {
        let enc = encoding_from_code_page("ANSI_1252");
        assert_eq!(enc, Some(encoding_rs::WINDOWS_1252));
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(
            encoding_from_code_page("ansi_1251"),
            encoding_from_code_page("ANSI_1251")
        );
    }

    #[test]
    fn test_ascii_returns_none() {
        assert_eq!(encoding_from_code_page("ASCII"), None);
    }

    #[test]
    fn test_utf8_returns_none() {
        assert_eq!(encoding_from_code_page("UTF-8"), None);
    }

    #[test]
    fn test_unknown_returns_windows1252() {
        let enc = encoding_from_code_page("SOMETHING_UNKNOWN");
        assert_eq!(enc, Some(encoding_rs::WINDOWS_1252));
    }

    #[test]
    fn test_asian_encodings() {
        assert_eq!(encoding_from_code_page("GB2312"), Some(encoding_rs::GBK));
        assert_eq!(encoding_from_code_page("BIG5"), Some(encoding_rs::BIG5));
        assert_eq!(encoding_from_code_page("ANSI_932"), Some(encoding_rs::SHIFT_JIS));
        assert_eq!(encoding_from_code_page("KOREAN"), Some(encoding_rs::EUC_KR));
    }
}
