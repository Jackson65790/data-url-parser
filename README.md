# data-url-parser

Parses `data:` URIs into a MIME type, a base64 flag, and the decoded payload
bytes.

```rust
use data_url_parser::parse_data_url;

let d = parse_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
assert_eq!(d.mime, "text/plain");
assert!(d.is_base64);
assert_eq!(d.data, b"hello");

let plain = parse_data_url("data:,hello%20world").unwrap();
assert_eq!(plain.mime, "text/plain");
assert!(!plain.is_base64);
assert_eq!(plain.data, b"hello world");
```

## Why this exists

The `data:` URL is a small format with a surprising number of sharp corners:
the media type is optional, the `;base64` marker is case-insensitive and must
be the last header segment, the comma is the first structural delimiter even
though commas are legal in the payload, and the two payload encodings
(base64 and percent-encoding) have different rules. This crate parses all of
that with no dependencies, returning owned values so the result can be moved
freely.

The main trade-off is strictness. We reject malformed input rather than
silently repairing it: a bad percent escape is an error, and base64 payloads
may not contain whitespace. That keeps the failure mode predictable, at the
cost of rejecting some technically-invalid-but-common URLs that permissive
parsers would accept.

## The awkward edge

A `+` in a non-base64 payload is a literal plus, not a space. The
`application/x-www-form-urlencoded` convention does not apply to data URLs, and
treating `+` as a space would corrupt binary payloads. If you are decoding a
form-encoded body, use a form decoder instead.

## API

- `parse_data_url(input: &str) -> Result<DataUrl, DataUrlError>`
- `DataUrl { mime: String, is_base64: bool, data: Vec<u8> }`
- `DataUrlError` with variants `MissingScheme`, `MissingComma`, `InvalidBase64`,
  and `InvalidPercentEscape`.
