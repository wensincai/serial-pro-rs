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
    #[error("Lua 钩子错误: {0}")]
    Hook(String),
    #[error("无效参数: {0}")]
    InvalidParam(String),
}

impl From<mlua::Error> for Error {
    fn from(e: mlua::Error) -> Self {
        Error::Hook(format!("{e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
