//! Target-independent handling of a response body read in one piece.
//!
//! `WorkersHttpClient` reads a body that is not a JavaScript stream at once.
//! This module decides what that read means, so it is tested natively even
//! though the client itself exists only on wasm32.

use incurs::outbound::HttpClientError;

/// The result of reading a whole body at once: the bytes, or the read
/// failure as a transport error. A failed read is never an empty body.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn buffered_body<E: std::fmt::Display>(
    read: Result<Vec<u8>, E>,
) -> Result<Vec<u8>, HttpClientError> {
    read.map_err(|error| HttpClientError::transport_message(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::buffered_body;

    #[test]
    fn a_failed_buffered_read_is_a_transport_error_not_an_empty_body() {
        let error = buffered_body::<&str>(Err("body stream reset")).unwrap_err();
        assert_eq!(error.code(), "HTTP_TRANSPORT_ERROR");
        assert!(error.retryable());
        assert_eq!(error.to_string(), "body stream reset");
        assert_eq!(
            buffered_body::<&str>(Ok(b"ok".to_vec())).unwrap(),
            b"ok".to_vec()
        );
    }
}
