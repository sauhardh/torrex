use core::error;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::utils::config::config_dir;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum DownloadState {
    Downloading,
    Paused,
    Stopped,
    Completed,
    Error,
    Initialized,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum DownloadCommand {
    Start,
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone)]
pub struct DownloadManager {
    /// Download state of the current downloading
    pub dl_state: Arc<RwLock<DownloadState>>,
    /// A Sender to control the Download State
    ///
    /// A [`DownloadCommand`] is passed through the channel by this sender.
    pub control_tx: Arc<Mutex<Option<Sender<DownloadCommand>>>>,
    /// A receiver to control the Download State
    ///
    /// A [DownloadCommand] received by this Receiver is to be reflected in the downlaod state and the nature of download
    pub control_rx: Arc<Mutex<Option<Receiver<DownloadCommand>>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            // Download state
            dl_state: Arc::new(RwLock::new(DownloadState::Initialized)),
            control_tx: Arc::new(Mutex::new(None)),
            control_rx: Arc::new(Mutex::new(None)),
        }
    }

    #[inline]
    pub async fn update_state_downloading(&self) {
        let mut state = self.dl_state.write().await;
        *state = DownloadState::Downloading;
    }

    #[inline]
    pub async fn update_state_completed(&self) {
        let mut state = self.dl_state.write().await;
        *state = DownloadState::Completed;
    }

    #[inline]
    pub async fn update_state_paused(&self) {
        let mut state = self.dl_state.write().await;
        *state = DownloadState::Paused;
    }

    #[inline]
    pub async fn update_state_stopped(&self) {
        let mut state = self.dl_state.write().await;
        *state = DownloadState::Stopped;
    }

    #[inline]
    pub async fn update_state_error(&self) {
        let mut state = self.dl_state.write().await;
        *state = DownloadState::Error;
    }

    // #[inline]
    // pub async fn update_state_resumed(&self) {
    //     let mut state = self.dl_state.write().await;
    //     *state = DownloadState::Resumed;
    // }

    #[inline]
    pub async fn get_download_state(&self) -> DownloadState {
        let state = self.dl_state.read().await;
        state.clone()
    }

    /// To pause the download
    pub async fn pause(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("pause function called");
        let state = self.get_download_state().await;
        if state == DownloadState::Paused {
            return Ok(());
        }

        self.update_state_paused().await;
        println!("pause updated");

        // TODO:
        // it may not be needed
        //
        if let Some(control_tx) = self.control_tx.lock().await.clone() {
            control_tx.send(DownloadCommand::Pause).await?;
        };

        Ok(())
    }

    /// To resume the download
    pub async fn resume(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("calling resume function");
        let state = self.get_download_state().await;
        if state == DownloadState::Downloading {
            return Ok(());
        }

        if let Some(control_tx) = self.control_tx.lock().await.clone() {
            control_tx.send(DownloadCommand::Resume).await?;
        }

        // self.update_state_resumed().await;
        self.update_state_downloading().await;

        Ok(())
    }

    /// To stop the download
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("stop called");
        let state = self.get_download_state().await;
        if state == DownloadState::Stopped {
            return Ok(());
        }

        self.update_state_stopped().await;
        println!("Download state updated");

        // TODO:
        // it may not be needed
        //
        if let Some(control_tx) = self.control_tx.lock().await.clone() {
            control_tx.send(DownloadCommand::Stop).await?;
        }

        Ok(())
    }

    pub async fn get_torrex_config_dir(
        &self,
        filename: String,
    ) -> Result<PathBuf, Box<dyn error::Error + Send + Sync>> {
        let path = config_dir("torrex").join(format!("{}.json", filename));

        if !path.exists() {
            File::create(path.clone()).await?;
        } else {
            return Err("Could not find the requested file/dir in the given path".into());
        }

        Ok(path)
    }

    pub async fn save_download_state(
        &self,
        completed_pieces: HashSet<u32>,
        destination: String,
        info_hash: String,
        peer_pieces: HashMap<String, Vec<u32>>,
    ) -> Result<(), Box<dyn error::Error + Send + Sync>> {
        let state = serde_json::json!({
            "completed_pieces": completed_pieces,
            "file_path": destination,
            "info_hash": info_hash,
            "history_state": self.dl_state.read().await.clone(),
            "peer_pieces": peer_pieces
        });

        match self.get_torrex_config_dir(info_hash).await {
            Ok(path) => {
                let file = File::options()
                    .create(true)
                    .write(true)
                    .read(true)
                    .open(path)
                    .await;

                if let Ok(mut file) = file {
                    let b = serde_json::to_vec_pretty(&state).unwrap();
                    file.write_all(&b).await?;
                }
            }
            Err(e) => {
                return Err(
                    format!("Failed to get torrex_dir while saving downloaded state. {e}").into(),
                );
            }
        }

        Ok(())
    }

    pub async fn load_download_state(
        &self,
        info_hash: String,
    ) -> Result<Option<serde_json::Value>, Box<dyn error::Error>> {
        match self.get_torrex_config_dir(info_hash.clone()).await {
            Ok(path) => {
                if let Ok(content) = tokio::fs::read_to_string(path).await {
                    let state: serde_json::Value = serde_json::from_str(&content)?;
                    // Todo:
                    // 1) resume downloading
                    return Ok(Some(state));
                }
            }
            Err(e) => {
                return Err(format!(
                    "Failed to get torrex_dir while loading downloaded state. {e}"
                )
                .into());
            }
        }

        Ok(None)
    }
}
