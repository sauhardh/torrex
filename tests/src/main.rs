use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .expect("Usage: cargo run -- ws://localhost:PORT/torrex/api/v1/ws/download/UUID");

    let (ws_stream, _) = connect_async(&url)
        .await
        .expect("Could not connect to websocket");

    println!("Connected to WebSocket server at {}", url);

    let (write, mut read) = ws_stream.split();

    while let Some(message) = read.next().await {
        match message {
            Ok(Message::Text(text)) => {
                println!("Got text: {text:?}");
            }
            Ok(Message::Binary(binary)) => {
                println!("Got binary: {binary:?}");
            }
            Ok(Message::Frame(frame)) => {
                println!("Got frame: {frame:?}");
            }
            Ok(Message::Close(close)) => {
                println!("Got close: {close:?}");
            }

            Ok(msg) => {
                println!("Got msg {msg:?}");
            }

            Err(e) => {
                eprintln!("Got error: {e:?}");
                break;
            }
        }
    }

    println!("Connection closed");
}
