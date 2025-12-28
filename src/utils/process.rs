use std::io::{Read, Write};
use std::mem::discriminant;
use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::NamedTempFile;

use crate::utils::file_watcher::ensure_watcher_initialized;
use crate::{config::FlockProcessConfig, utils::file_watcher::DebounceTimer};

pub struct ProcessState {
    pub process_config: FlockProcessConfig,
    pub status: ProcessStatus,
}

impl ProcessState {
    pub fn new(process_config: FlockProcessConfig) -> Self {
        Self {
            process_config,
            status: ProcessStatus::Stopped,
        }
    }

    pub fn launch(&mut self) -> Result<()> {
        if self.status == ProcessStatus::Stopped {
            self.status = ProcessStatus::Running(Process::new(&self.process_config)?);
        }

        // TODO need to think about this when refactoring graceful restart
        if let ProcessStatus::Running(process) = &mut self.status {
            if let ProcessRunningStatus::Restarting = process.status {
                self.status = ProcessStatus::Running(Process::new(&self.process_config)?);
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
pub enum ProcessStatus {
    Stopped,
    Running(Process),
}

impl PartialEq for ProcessStatus {
    fn eq(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }
}

#[derive(Clone)]
pub struct Process {
    pub child: Arc<RwLock<Box<dyn portable_pty::Child + Send + Sync>>>,
    pub pty_master: Arc<Box<dyn portable_pty::MasterPty + Send>>,
    pub parser: Arc<RwLock<vt100::Parser>>,
    pub status: ProcessRunningStatus,
}

impl Process {
    pub fn new(process_config: &FlockProcessConfig) -> Result<Self> {
        // Initialize watcher lazily if this is a watchable process
        if process_config.watch.is_enabled() {
            ensure_watcher_initialized();
        }

        // Launch the process using PTY for proper interactive support
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("Failed to open PTY: {}", e))?;

        let mut script = NamedTempFile::new()?;
        let script_path = script.path().display().to_string();
        writeln!(script, "{}", process_config.command)?;
        let _ = script.persist(script_path.clone());

        // Use the login shell from SHELL environment variable, fallback to sh
        let shell = std::env::var("SHELL").unwrap_or("sh".to_string());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg(script_path);
        cmd.cwd(std::env::current_dir()?);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("Failed to spawn command: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow!("Failed to clone PTY reader: {}", e))?;

        // Create a VT100 parser to handle terminal escape sequences
        let parser = Arc::new(RwLock::new(vt100::Parser::new(24, 80, 0)));
        let parser_clone = parser.clone();

        std::thread::spawn(move || {
            loop {
                let mut buffer = [0; 8192];
                let bytes_read = match reader.read(&mut buffer) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if bytes_read == 0 {
                    break;
                }
                // Feed the output to the VT100 parser
                parser_clone.write().unwrap().process(&buffer[..bytes_read]);
            }
        });

        Ok(Self {
            child: Arc::new(RwLock::new(child)),
            pty_master: Arc::new(pair.master),
            parser,
            status: ProcessRunningStatus::Stable,
        })
    }
}

#[derive(Clone)]
pub enum ProcessRunningStatus {
    Stable,
    Debouncing(DebounceTimer),
    Restarting,
}

impl PartialEq for ProcessRunningStatus {
    fn eq(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }
}
