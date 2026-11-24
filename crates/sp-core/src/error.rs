use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("串口错误: {0}")]
    Serial(String),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("无效的 Hex: {0}")]
    InvalidHex(String),
}

pub type Result<T> = std::result::Result<T, Error>;
