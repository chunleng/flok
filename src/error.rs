use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlokError {
    #[error("Error while processing config\n{0}")]
    Config(#[from] FlokConfigError),

    #[error("Error while running the program\n{0}")]
    Program(#[from] FlokProgramError),
}

#[derive(Debug, Error)]
pub enum FlokConfigError {
    #[error("{0}")]
    Known(#[from] anyhow::Error),
    #[error("An unknown IO error has occured: {0}")]
    UnknownStdIo(#[from] std::io::Error),
    #[error("An unknown SerDe error has occured: {0}")]
    UnknownSerDe(#[from] serde_yaml::Error),
    #[error("Program crashed due to an unknown error")]
    Unknown(#[from] Box<dyn std::error::Error>),
}

#[derive(Debug, Error)]
pub enum FlokProgramError {
    #[error("During initiation: {0}")]
    Init(FlokProgramInitError),
    #[error("While programming is running: {0}")]
    Execution(FlokProgramExecutionError),
}

#[derive(Debug, Error)]
pub enum FlokProgramInitError {
    #[error("An unknown IO error has occured: {0}")]
    UnknownStdIo(#[from] std::io::Error),
    #[error("Program crashed due to an unknown error")]
    Unknown(#[from] Box<dyn std::error::Error>),
}

#[derive(Debug, Error)]
pub enum FlokProgramExecutionError {
    #[error("{0}")]
    Known(#[from] anyhow::Error),
    #[error("An unknown IO error has occured: {0}")]
    UnknownStdIo(#[from] std::io::Error),
}
