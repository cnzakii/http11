use std::error::Error;
use std::fmt;

/// An HTTP request method token.
///
/// Methods are case-sensitive and extensible. Construction accepts any
/// non-empty token defined by HTTP semantics and preserves its original bytes.
/// See [RFC 9110 Section 9.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.1).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Method(Box<[u8]>);

impl Method {
    /// Validates and copies an HTTP method token.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMethod`] when `value` is empty or contains a byte that
    /// is not allowed in an HTTP token.
    pub fn from_bytes(value: &[u8]) -> Result<Self, InvalidMethod> {
        if value.is_empty() || !value.iter().copied().all(is_token_byte) {
            return Err(InvalidMethod);
        }

        Ok(Self(value.into()))
    }

    /// Returns the method token exactly as supplied during construction.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Method {
    type Error = InvalidMethod;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(value)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = std::str::from_utf8(self.as_bytes()).map_err(|_| fmt::Error)?;
        formatter.write_str(value)
    }
}

/// An error returned when bytes are not a valid HTTP method token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidMethod;

impl fmt::Display for InvalidMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HTTP method must be a non-empty token")
    }
}

impl Error for InvalidMethod {}

pub(crate) const fn is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}

#[cfg(test)]
mod tests {
    use super::Method;

    #[test]
    fn accepts_standard_and_extension_methods_without_normalizing_them() {
        for value in [b"GET".as_slice(), b"M-SEARCH", b"custom"] {
            let method = Method::from_bytes(value).expect("valid method");
            assert_eq!(method.as_bytes(), value);
        }

        assert_ne!(
            Method::from_bytes(b"GET").expect("valid method"),
            Method::from_bytes(b"get").expect("valid method")
        );

        let mut input = b"CUSTOM".to_vec();
        let method = Method::from_bytes(&input).expect("valid method");
        input[0] = b'X';
        assert_eq!(method.as_bytes(), b"CUSTOM");
    }

    #[test]
    fn accepts_exactly_non_empty_http_tokens() {
        assert!(Method::from_bytes(b"").is_err());

        for byte in u8::MIN..=u8::MAX {
            let expected = byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
            assert_eq!(Method::from_bytes(&[byte]).is_ok(), expected);
        }
    }
}
