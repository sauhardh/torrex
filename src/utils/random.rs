// This mod generate random string of length 20 for peer_id

use rand;
use rand::{Rng, distr::Alphanumeric}; // 0.8

pub fn generate_peerid() -> String {
    let s: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect();

    s
}
