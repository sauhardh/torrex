use sha1::digest::generic_array::GenericArray;
use sha1::digest::generic_array::typenum::U20;
use sha1::{Digest, Sha1};

pub fn sha1_hash(data: &[u8]) -> GenericArray<u8, U20> {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize()
}
