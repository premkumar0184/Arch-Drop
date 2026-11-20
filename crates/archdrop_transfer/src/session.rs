use crate::error::TransferError;
use archdrop_core::models::dto_v2::{FileDto, PrepareUploadResponseDtoV2, RegisterDtoV2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug)]
pub struct FileTransferState {
    pub dto: FileDto,
    pub token: String,
    pub temp_path: PathBuf,
    pub final_path: PathBuf,
    pub bytes_received: u64,
    pub status: FileTransferStatus,
}

#[derive(Debug)]
pub struct TransferSession {
    pub session_id: String,
    pub sender_info: RegisterDtoV2,
    pub files: HashMap<String, FileTransferState>,
    pub destination_dir: PathBuf,
    pub created_at: std::time::Instant,
}

#[derive(Clone, Debug)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, TransferSession>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Sanitizes the advertised filename and resolves collision in destination directory.
    pub fn resolve_destination_path(dest_dir: &Path, advertised_name: &str) -> PathBuf {
        let raw_name = Path::new(advertised_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archdrop_received_file");

        let sanitized: String = raw_name
            .chars()
            .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
            .collect();

        let target = dest_dir.join(&sanitized);
        if !target.exists() {
            return target;
        }

        let stem = Path::new(&sanitized)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = Path::new(&sanitized)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();

        let mut counter = 1;
        loop {
            let candidate = dest_dir.join(format!("{} ({}){}", stem, counter, ext));
            if !candidate.exists() {
                return candidate;
            }
            counter += 1;
        }
    }

    /// Prepares a new upload transfer session, generating tokens and paths for each file.
    pub async fn create_session(
        &self,
        sender_info: RegisterDtoV2,
        files: HashMap<String, FileDto>,
        destination_dir: PathBuf,
    ) -> Result<PrepareUploadResponseDtoV2, TransferError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut response_files = HashMap::new();
        let mut session_files = HashMap::new();

        // Ensure destination directory exists
        tokio::fs::create_dir_all(&destination_dir).await?;

        for (file_id, file_dto) in files {
            let token = uuid::Uuid::new_v4().to_string();
            let final_path = Self::resolve_destination_path(&destination_dir, &file_dto.file_name);
            let temp_path = destination_dir.join(format!(
                ".{}.part.{}",
                final_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file"),
                &session_id[..8]
            ));

            response_files.insert(file_id.clone(), token.clone());
            session_files.insert(
                file_id,
                FileTransferState {
                    dto: file_dto,
                    token,
                    temp_path,
                    final_path,
                    bytes_received: 0,
                    status: FileTransferStatus::Pending,
                },
            );
        }

        let session = TransferSession {
            session_id: session_id.clone(),
            sender_info,
            files: session_files,
            destination_dir,
            created_at: std::time::Instant::now(),
        };

        self.sessions.write().await.insert(session_id.clone(), session);
        info!(session_id = %session_id, "Created new transfer session with {} files", response_files.len());

        Ok(PrepareUploadResponseDtoV2 {
            session_id,
            files: response_files,
        })
    }

    /// Validates an incoming upload request credentials.
    pub async fn validate_upload_token(
        &self,
        session_id: &str,
        file_id: &str,
        token: &str,
    ) -> Result<(PathBuf, PathBuf, u64, Option<String>), TransferError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TransferError::SessionNotFound(session_id.to_string()))?;

        let file_state = session
            .files
            .get_mut(file_id)
            .ok_or_else(|| TransferError::FileNotFound(file_id.to_string()))?;

        if file_state.token != token {
            warn!(session_id = %session_id, file_id = %file_id, "Invalid upload token provided");
            return Err(TransferError::InvalidToken);
        }

        file_state.status = FileTransferStatus::InProgress;
        Ok((
            file_state.temp_path.clone(),
            file_state.final_path.clone(),
            file_state.dto.size,
            file_state.dto.sha256.clone(),
        ))
    }

    /// Marks a file as completed and updates state.
    pub async fn mark_file_completed(
        &self,
        session_id: &str,
        file_id: &str,
        bytes_received: u64,
    ) -> Result<(), TransferError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TransferError::SessionNotFound(session_id.to_string()))?;

        let file_state = session
            .files
            .get_mut(file_id)
            .ok_or_else(|| TransferError::FileNotFound(file_id.to_string()))?;

        file_state.bytes_received = bytes_received;
        file_state.status = FileTransferStatus::Completed;
        info!(
            session_id = %session_id,
            file_id = %file_id,
            final_path = ?file_state.final_path,
            bytes = bytes_received,
            "File transfer marked COMPLETED"
        );
        Ok(())
    }

    /// Marks a file as failed and deletes its temporary part file.
    pub async fn mark_file_failed(
        &self,
        session_id: &str,
        file_id: &str,
        error: String,
    ) -> Result<(), TransferError> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            if let Some(file_state) = session.files.get_mut(file_id) {
                file_state.status = FileTransferStatus::Failed(error);
                let _ = tokio::fs::remove_file(&file_state.temp_path).await;
            }
        }
        Ok(())
    }

    /// Cancels a session and purges any temporary part files.
    pub async fn cancel_session(&self, session_id: &str) -> Result<(), TransferError> {
        let mut sessions = self.sessions.write().await;
        if let Some(mut session) = sessions.remove(session_id) {
            info!(session_id = %session_id, "Cancelling transfer session and purging temp files");
            for file_state in session.files.values_mut() {
                file_state.status = FileTransferStatus::Cancelled;
                let _ = tokio::fs::remove_file(&file_state.temp_path).await;
            }
        }
        Ok(())
    }

    /// Checks if all files in a session are completed.
    pub async fn is_session_completed(&self, session_id: &str) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            session
                .files
                .values()
                .all(|f| f.status == FileTransferStatus::Completed)
        } else {
            false
        }
    }

    /// Returns the advertised alias of the sender for the given session ID, if found.
    pub async fn get_peer_alias(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| s.sender_info.alias.clone())
    }
}
