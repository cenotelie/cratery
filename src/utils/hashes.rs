use data_encoding::HEXLOWER;
use ring::digest::{Context, SHA256};

/// Computes the SHA256 digest of bytes
pub fn sha256(buffer: &[u8]) -> String {
    let mut context = Context::new(&SHA256);
    context.update(buffer);
    let digest = context.finish();
    HEXLOWER.encode(digest.as_ref())
}
