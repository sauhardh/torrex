use hex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_bencode::value::Value;

use std::collections::HashSet;
use std::fmt::Write;
use std::{borrow::Cow, collections::HashMap};

use crate::{
    bencode::Bencode, extension::magnet_link::Parser, handshake::Handshake, peers::ResponseParams,
    utils,
};

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct MagnetPeerInfo {
    #[serde(rename = "peer id")]
    pub peer_id: Vec<u8>,
    pub ip: String,
    pub port: u32,

    #[serde(skip_serializing, skip_deserializing)]
    pub peer_id_hex: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct MagnetResponseParams {
    pub interval: u8,
    #[serde(rename = "min interval")]
    pub min_interval: u8,
    pub peers: Vec<MagnetPeerInfo>,
}

#[derive(Debug)]
pub struct ExtendedExchange<'a> {
    peers: Vec<String>,
    handshake: Handshake,
    pub parser: &'a mut Parser,
    response: MagnetResponseParams,
}

impl<'a> ExtendedExchange<'a> {
    pub fn new(parser: &'a mut Parser) -> Self {
        Self {
            peers: Vec::new(),
            handshake: Handshake::default(),
            parser,
            response: MagnetResponseParams::default(),
        }
    }

    pub async fn get_peers(&mut self, peer_id: String) -> &mut Self {
        let client = reqwest::Client::new();
        let mut url = Url::parse(&self.parser.magnet_link.tr.as_mut().unwrap().to_string())
            .expect("Could not parse provided url");

        let info_hash =
            hex::decode(&self.parser.magnet_link.xt).expect("Could not decode provided info_hash");

        url.query_pairs_mut()
            .encoding_override(Some(&|input| {
                if input == "{{info_hash}}" {
                    Cow::Owned(info_hash.clone())
                } else {
                    Cow::Borrowed(input.as_bytes())
                }
            }))
            .append_pair("info_hash", "{{info_hash}}")
            .append_pair("peer_id", &peer_id)
            .append_pair("left", "999")
            .append_pair("downloaded", "0")
            .append_pair("uploaded", "0")
            .append_pair("port", "6881")
            .append_pair("compact", "0");

        match client.get(url.clone()).send().await {
            Ok(res) => {
                if let Ok(buf) = res.bytes().await {
                    let res = Bencode::new().decoder(&buf).0;
                    self.response = serde_json::from_value::<MagnetResponseParams>(
                        Bencode::new().decoder(&buf).0,
                    )
                    .unwrap();
                }
            }
            Err(e) => {
                eprintln!("Failed to connect with provided url.. FurtherMore: {e}");
            }
        }

        self
    }

    pub fn include_hex(&mut self) -> &mut Self {
        for peer_info in &mut self.response.peers {
            peer_info.peer_id_hex = Some(hex::encode(&peer_info.peer_id));
        }

        self
    }

    pub fn peers_ip(&mut self) -> Vec<String> {
        let mut ips: Vec<String> = Vec::new();
        for peer_info in &mut self.response.peers {
            let addr = format!("{}:{}", peer_info.ip, peer_info.port);
            ips.push(addr);
        }

        ips
    }
}

#[cfg(test)]
mod test_extdhandshake {

    use crate::connection::connection::PeerConnection;
    use crate::connection::connection::SwarmManager;
    use crate::connection::handshake;
    use crate::extension::magnet_link::Parser;
    use crate::peers::Peers;

    use super::*;
    use utils::random;

    #[tokio::test]
    async fn test() {
        let url: String = "magnet:?xt=urn:btih:3f994a835e090238873498636b98a3e78d1c34ca&dn=magnet2.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
        let mut parser = Parser::new(url.clone());
        let parser = parser.parse();

        let peer_id = random::generate_magnet_peerid();
        let mut extd_exchange = ExtendedExchange::new(parser);
        let extd_exchange = extd_exchange.get_peers(peer_id.clone()).await.include_hex();

        let ips = extd_exchange.peers_ip();
        println!("ips: {ips:?}");

        println!("extd_exchange {:?}", extd_exchange);
        let info_hash = &parser.magnet_link.xt;

        let mut dm = SwarmManager::init();
        dm.connect_and_exchange_bitfield(
            ips,
            info_hash.as_bytes().to_vec(),
            peer_id.as_bytes().to_vec(),
        )
        .await;
    }
}
