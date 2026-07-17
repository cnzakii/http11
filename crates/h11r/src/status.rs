use std::error::Error;
use std::fmt;

/// An HTTP status code in the protocol-valid range `100..=599`.
///
/// See [RFC 9110 Section 15](https://www.rfc-editor.org/rfc/rfc9110.html#section-15).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StatusCode(u16);

impl StatusCode {
    /// Creates an HTTP status code from its numeric representation.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStatusCode`] when `value` is outside `100..=599`.
    pub const fn from_u16(value: u16) -> Result<Self, InvalidStatusCode> {
        if value >= 100 && value <= 599 {
            Ok(Self(value))
        } else {
            Err(InvalidStatusCode)
        }
    }

    /// Returns the numeric status code.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Returns whether this is an informational (`1xx`) status.
    #[must_use]
    pub const fn is_informational(self) -> bool {
        self.0 < 200
    }

    /// Returns whether this is a successful (`2xx`) status.
    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    /// Returns whether this is a redirection (`3xx`) status.
    #[must_use]
    pub const fn is_redirection(self) -> bool {
        self.0 >= 300 && self.0 < 400
    }

    /// Returns whether this is a client-error (`4xx`) status.
    #[must_use]
    pub const fn is_client_error(self) -> bool {
        self.0 >= 400 && self.0 < 500
    }

    /// Returns whether this is a server-error (`5xx`) status.
    #[must_use]
    pub const fn is_server_error(self) -> bool {
        self.0 >= 500
    }
}

impl TryFrom<u16> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::from_u16(value)
    }
}

impl From<StatusCode> for u16 {
    fn from(value: StatusCode) -> Self {
        value.as_u16()
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An error returned when an integer is outside the valid HTTP status range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidStatusCode;

impl fmt::Display for InvalidStatusCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HTTP status code must be between 100 and 599")
    }
}

impl Error for InvalidStatusCode {}

#[cfg(test)]
mod tests {
    use super::StatusCode;

    #[test]
    fn accepts_exactly_three_digit_protocol_status_codes() {
        assert!(StatusCode::from_u16(99).is_err());
        assert!(StatusCode::from_u16(600).is_err());

        for value in 100..=599 {
            assert_eq!(
                StatusCode::from_u16(value).expect("valid status").as_u16(),
                value
            );
        }
    }

    #[test]
    fn classifies_each_status_by_its_first_digit() {
        for value in 100..=599 {
            let status = StatusCode::from_u16(value).expect("valid status");
            assert_eq!(status.is_informational(), value / 100 == 1);
            assert_eq!(status.is_success(), value / 100 == 2);
            assert_eq!(status.is_redirection(), value / 100 == 3);
            assert_eq!(status.is_client_error(), value / 100 == 4);
            assert_eq!(status.is_server_error(), value / 100 == 5);
        }
    }
}
