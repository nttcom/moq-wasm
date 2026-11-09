pub mod canonical_object;
pub mod datagram_field;
pub mod decode_error;
pub mod extension_headers;
pub mod fetch;
pub mod object_datagram;
pub mod object_status;
pub mod subgroup;

use decode_error::DecodeError;

/// The largest Object Payload Length or Extension Headers Length a peer may
/// declare on a stream. A receiver can hold all of a declared length in
/// memory before it decodes, so a larger one is a decode error that ends
/// the stream (draft-14 §12.1: cancel a stream that reaches a resource limit).
const MAX_WIRE_DECLARED_LENGTH: u64 = 16 * 1024 * 1024;

/// Compares the `u64` before the cast, since `as usize` truncates on a 32-bit target.
fn declared_length(length: u64) -> Result<usize, DecodeError> {
    if length > MAX_WIRE_DECLARED_LENGTH {
        return Err(DecodeError::Fatal(format!(
            "declared length {length} exceeds {MAX_WIRE_DECLARED_LENGTH}"
        )));
    }
    Ok(length as usize)
}
