//! A small, dependency-free parser for `data:` URLs.
//!
//! The parser is deliberately strict about the grammar in RFC 2397: a data URL
//! is `data:[<mediatype>][;base64],<data>`. The only place we are lenient is
//! that we accept a missing media type and default it to `text/plain` without
//! the `charset=US-ASCII` parameter, which is what browsers do in practice and
//! what most callers actually want when they write `data:,hello`.
//!
//! The parser returns owned values. That avoids lifetime plumbing at the call
//! site and keeps the API usable from anywhere, at the cost of one allocation
//! per component. For a parser whose output is usually fed straight into a
//! decoder or a network stack this is the right trade-off.

use std::fmt;

/// A fully parsed data URL.
///
/// `mime` is the media type exactly as it appeared before the comma, minus the
/// `;base64` marker. `is_base64` records whether that marker was present.
/// `data` is the decoded payload: for base64 URLs it is the decoded bytes, for
/// plain URLs it is the percent-decoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrl {
    pub mime: String,
    pub is_base64: bool,
    pub data: Vec<u8>,
}

/// Errors that can occur while parsing a data URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataUrlError {
    /// The input did not start with `data:` (case-insensitive).
    MissingScheme,
    /// No comma was found separating the header from the payload.
    MissingComma,
    /// The `;base64` marker was present but the payload was not valid base64.
    InvalidBase64,
    /// A percent escape was malformed, e.g. `%`, `%A`, or `%ZZ`.
    InvalidPercentEscape,
}

impl fmt::Display for DataUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataUrlError::MissingScheme => write!(f, "data URL must start with 'data:'"),
            DataUrlError::MissingComma => write!(f, "data URL is missing the ',' separator"),
            DataUrlError::InvalidBase64 => write!(f, "payload is not valid base64"),
            DataUrlError::InvalidPercentEscape => write!(f, "payload contains a malformed percent escape"),
        }
    }
}

impl std::error::Error for DataUrlError {}

/// Parse a data URL into its components.
///
/// The scheme check is case-insensitive because RFC 3986 says URI schemes are
/// case-insensitive, and real documents do contain `DATA:`.
///
/// The media type is returned verbatim (no lowercasing, no parameter parsing)
/// because callers that care about parameters can split on `;` themselves, and
/// callers that don't should not pay for it. The `;base64` marker is detected
/// case-insensitively as well, since the token is defined as case-insensitive.
///
/// A missing media type yields `text/plain`. An empty media type (e.g.
/// `data:;base64,...`) also yields `text/plain`; the empty string is never a
/// useful media type and treating it as the default matches browser behavior.
///
/// # Errors
///
/// Returns [`DataUrlError`] if the scheme is missing, the comma is missing,
/// the base64 payload is invalid, or a percent escape in a non-base64 payload
/// is malformed.
///
/// # Examples
///
/// ```
/// use data_url_parser::parse_data_url;
///
/// let d = parse_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
/// assert_eq!(d.mime, "text/plain");
/// assert!(d.is_base64);
/// assert_eq!(d.data, b"hello");
/// ```
pub fn parse_data_url(input: &str) -> Result<DataUrl, DataUrlError> {
    // The scheme is exactly 5 bytes: `data:`. Comparing the first five bytes
    // case-insensitively avoids allocating a lowercased copy of the whole URL.
    if input.len() < 5 || !input[..5].eq_ignore_ascii_case("data:") {
        return Err(DataUrlError::MissingScheme);
    }
    let rest = &input[5..];

    // The comma is the first structural delimiter. A comma inside the payload
    // is legal (it is just data), so we must take the *first* comma, not the
    // last. `find` does exactly that.
    let comma = rest.find(',').ok_or(DataUrlError::MissingComma)?;
    let header = &rest[..comma];
    let payload = &rest[comma + 1..];

    // Split off a trailing `;base64` marker. We check the last segment rather
    // than the first so that a media type parameter such as
    // `text/plain;charset=base64` is not misread as the base64 marker.
    let (mime, is_base64) = split_base64_marker(header);

    let data = if is_base64 {
        decode_base64(payload.as_bytes()).ok_or(DataUrlError::InvalidBase64)?
    } else {
        percent_decode(payload.as_bytes())?
    };

    Ok(DataUrl {
        mime: if mime.is_empty() {
            "text/plain".to_string()
        } else {
            mime.to_string()
        },
        is_base64,
        data,
    })
}

/// Split `header` into `(media_type, is_base64)`.
///
/// The base64 marker is only recognised as the final `;base64` segment. A
/// header like `text/plain;base64=1` is a parameter, not a marker, and is left
/// in the media type. This is the interpretation browsers use and it avoids the
/// ambiguity of a media type that happens to contain the word `base64`.
fn split_base64_marker(header: &str) -> (&str, bool) {
    if let Some(idx) = header.rfind(';') {
        let tail = &header[idx + 1..];
        if tail.eq_ignore_ascii_case("base64") {
            return (&header[..idx], true);
        }
    }
    (header, false)
}

/// Decode a standard (not URL-safe) base64 payload.
///
/// Padding is optional: the decoder accepts both padded and unpadded input,
/// because data URLs in the wild are written both ways. Whitespace is rejected
/// rather than skipped, because a data URL payload is not a MIME body and
/// silently ignoring whitespace hides malformed input.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    // Strip an optional single trailing '=' padding pair. We do not require it.
    let mut end = input.len();
    if end > 0 && input[end - 1] == b'=' {
        end -= 1;
    }
    if end > 0 && input[end - 1] == b'=' {
        end -= 1;
    }
    let body = &input[..end];

    // A base64 body must not have a length congruent to 1 mod 4; that
    // corresponds to a stray 6-bit group with no partner.
    if body.len() % 4 == 1 {
        return None;
    }

    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &b in body {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // Any leftover bits must be zero padding; otherwise the input carried
    // information that cannot be represented in whole bytes.
    if bits > 0 && (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// Percent-decode a payload byte-by-byte.
///
/// `+` is left as a literal plus. That is the correct choice for data URLs:
/// the `application/x-www-form-urlencoded` convention of treating `+` as a
/// space does not apply here, and decoding it as a space would corrupt binary
/// payloads.
fn percent_decode(input: &[u8]) -> Result<Vec<u8>, DataUrlError> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            if i + 2 >= input.len() {
                return Err(DataUrlError::InvalidPercentEscape);
            }
            let hi = hex_value(input[i + 1]).ok_or(DataUrlError::InvalidPercentEscape)?;
            let lo = hex_value(input[i + 2]).ok_or(DataUrlError::InvalidPercentEscape)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Map an ASCII hex digit to its value, or `None` if it is not a hex digit.
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_text() {
        let d = parse_data_url("data:,hello").unwrap();
        assert_eq!(d.mime, "text/plain");
        assert!(!d.is_base64);
        assert_eq!(d.data, b"hello");
    }

    #[test]
    fn parses_explicit_media_type() {
        let d = parse_data_url("data:text/html,<p>hi</p>").unwrap();
        assert_eq!(d.mime, "text/html");
        assert!(!d.is_base64);
        assert_eq!(d.data, b"<p>hi</p>");
    }

    #[test]
    fn parses_base64() {
        let d = parse_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(d.mime, "text/plain");
        assert!(d.is_base64);
        assert_eq!(d.data, b"hello");
    }

    #[test]
    fn accepts_unpadded_base64() {
        // `aGVsbG8` is `hello` without the padding.
        let d = parse_data_url("data:;base64,aGVsbG8").unwrap();
        assert_eq!(d.mime, "text/plain");
        assert!(d.is_base64);
        assert_eq!(d.data, b"hello");
    }

    #[test]
    fn base64_marker_is_case_insensitive() {
        let d = parse_data_url("data:;BASE64,aGVsbG8=").unwrap();
        assert!(d.is_base64);
        assert_eq!(d.data, b"hello");
    }

    #[test]
    fn scheme_is_case_insensitive() {
        let d = parse_data_url("DATA:,hi").unwrap();
        assert_eq!(d.mime, "text/plain");
        assert_eq!(d.data, b"hi");
    }

    #[test]
    fn missing_scheme_is_rejected() {
        assert_eq!(parse_data_url("http://example").unwrap_err(), DataUrlError::MissingScheme);
    }

    #[test]
    fn missing_comma_is_rejected() {
        assert_eq!(parse_data_url("data:text/plain").unwrap_err(), DataUrlError::MissingComma);
    }

    #[test]
    fn invalid_base64_is_rejected() {
        assert_eq!(
            parse_data_url("data:;base64,!!!!").unwrap_err(),
            DataUrlError::InvalidBase64
        );
    }

    #[test]
    fn percent_decodes_non_base64_payload() {
        let d = parse_data_url("data:,a%20b%2Fc").unwrap();
        assert_eq!(d.data, b"a b/c");
    }

    #[test]
    fn plus_is_not_decoded_as_space() {
        let d = parse_data_url("data:,a+b").unwrap();
        assert_eq!(d.data, b"a+b");
    }

    #[test]
    fn malformed_percent_escape_is_rejected() {
        assert_eq!(
            parse_data_url("data:,abc%").unwrap_err(),
            DataUrlError::InvalidPercentEscape
        );
        assert_eq!(
            parse_data_url("data:,abc%G0").unwrap_err(),
            DataUrlError::InvalidPercentEscape
        );
    }

    #[test]
    fn comma_inside_payload_is_data() {
        let d = parse_data_url("data:,a,b,c").unwrap();
        assert_eq!(d.data, b"a,b,c");
    }

    #[test]
    fn media_type_parameter_is_preserved() {
        let d = parse_data_url("data:text/plain;charset=utf-8,hi").unwrap();
        assert_eq!(d.mime, "text/plain;charset=utf-8");
        assert!(!d.is_base64);
        assert_eq!(d.data, b"hi");
    }

    #[test]
    fn base64_marker_must_be_final_segment() {
        // `base64` here is a parameter value, not the marker, so the payload is
        // treated as plain text.
        let d = parse_data_url("data:text/plain;base64=1,hi").unwrap();
        assert_eq!(d.mime, "text/plain;base64=1");
        assert!(!d.is_base64);
        assert_eq!(d.data, b"hi");
    }

    #[test]
    fn empty_payload_is_empty_bytes() {
        let d = parse_data_url("data:,").unwrap();
        assert_eq!(d.mime, "text/plain");
        assert_eq!(d.data, b"");
    }
}
