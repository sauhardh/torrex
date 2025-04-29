// This mod generate random string of length 20 for peer_id

use rand;
use rand::Rng;
use rand::distr::Alphabetic; // 0.8

pub fn generate_peerid() -> String {
    let s: String = rand::rng()
        .sample_iter(&Alphabetic)
        .take(20)
        .map(char::from)
        .collect();

    s
}
