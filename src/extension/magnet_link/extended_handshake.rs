use hex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_bencode::value::Value;

use std::collections::HashSet;
use std::fmt::Write;
use std::{borrow::Cow, collections::HashMap};

use crate::peers::{Event, Peers, RequestParams};
use crate::{
    bencode::Bencode, extension::magnet_link::Parser, handshake::Handshake, peers::ResponseParams,
    utils,
};

#[derive(Debug)]
pub struct ExtendedExchange<'a> {
    pub parser: &'a mut Parser,
    peers: Peers,
}

impl<'a> ExtendedExchange<'a> {
    pub fn new(parser: &'a mut Parser) -> Self {
        Self {
            parser,
            peers: Peers::default(),
        }
    }

    pub fn set_request(
        &mut self,
        info_hash: Vec<u8>,
        peer_id: String,
        ip: Option<String>,
        port: usize,
        uploaded: usize,
        downloaded: usize,
        left: usize,
        event: Option<Event>,
        compact: u32,
    ) -> &mut Self {
        let req = self.peers.request.new(
            info_hash.clone(),
            peer_id.clone(), // random string
            ip,
            port,
            uploaded,
            downloaded,
            left,
            event,
            compact,
        );

        self.peers.request = req;

        self
    }

    pub async fn request_tracker(&mut self) -> &Self {
        let request = self.peers.request.clone();
        self.peers.request_tracker(&request).await;

        self
    }

    pub fn set_url(&mut self, url: String) -> &mut Self {
        self.peers.announce_url(url);

        self
    }

    pub fn peers_ip(&self) -> Vec<String> {
        self.peers
            .response
            .as_ref()
            .map(|r| r.peers_ip())
            .unwrap_or_default()
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
        let mut parser = &mut Parser::new(url);
        parser = parser.parse();

        let info_hash = &parser.magnet_link.xt.clone();
        let announce_url = &parser
            .magnet_link
            .tr
            .clone()
            .expect("Tracker URL (tr=...) missing in magnet link");

        let mut extd_exchange = ExtendedExchange::new(parser);

        let info_hash = hex::decode(info_hash).expect("Could not decode provided info_hash");
        let peer_id = random::generate_magnet_peerid();

        let extd = extd_exchange
            .set_request(
                info_hash.clone(),
                peer_id.clone(), // random string
                None,
                6881,
                0,
                0,
                999,
                None,
                1,
            )
            .set_url(announce_url.to_string())
            .request_tracker()
            .await;

        let ips = extd.peers_ip();

        for addr in ips {
            if let Ok(mut conn) = PeerConnection::init(addr).await {
                match conn
                    .init_handshaking(info_hash.clone(), peer_id.as_bytes().to_vec())
                    .await
                {
                    Ok(remote_peer_id) => {
                        println!("Peer ID: {}", remote_peer_id);
                    }

                    Err(e) => {
                        eprintln!("Error occured on initiating handshake {e:?} ",);
                        return;
                    }
                }
            };
        }
    }
}
