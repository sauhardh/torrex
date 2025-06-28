/*
This module is to support weebseeding.
However, it is not currently used
*/

use reqwest::Client;
use std::fs::OpenOptions;
use std::io::{Seek, Write};

pub async fn download_piece_from_webseed(
    url: &str,
    piece_length: usize,
    file_path: &str,
    file_offset: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let range_header = format!(
        "bytes={}-{}",
        file_offset,
        file_offset + piece_length as u64 - 1,
    );

    let res = client
        .get(url)
        .header("Range", range_header)
        .send()
        .await?
        .error_for_status()?;

    let bytes = res.bytes().await?;

    let mut file = OpenOptions::new().write(true).open(file_path)?;
    file.seek(std::io::SeekFrom::Start(file_offset))?;
    file.write_all(&bytes)?;

    Ok(())
}
