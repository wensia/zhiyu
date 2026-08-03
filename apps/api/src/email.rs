use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EmailMessage {
    pub to: String,
    pub subject: String,
    pub text: String,
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send(&self, message: EmailMessage) -> Result<()>;
}

#[derive(Clone)]
pub struct DevFileEmailSender {
    directory: Arc<PathBuf>,
}

impl DevFileEmailSender {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: Arc::new(directory),
        }
    }
}

#[async_trait]
impl EmailSender for DevFileEmailSender {
    async fn send(&self, message: EmailMessage) -> Result<()> {
        tokio::fs::create_dir_all(self.directory.as_ref()).await?;
        let filename = format!(
            "{}-{}.eml",
            Utc::now().format("%Y%m%dT%H%M%S"),
            Uuid::now_v7()
        );
        let path = self.directory.join(filename);
        let mut file = tokio::fs::File::create(&path).await?;
        file.write_all(
            format!(
                "To: {}\nSubject: {}\nContent-Type: text/plain; charset=utf-8\n\n{}\n",
                message.to, message.subject, message.text
            )
            .as_bytes(),
        )
        .await?;
        file.flush().await?;
        tracing::info!(path = %path.display(), "development email captured");
        Ok(())
    }
}
