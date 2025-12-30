use std::io::{Read, Write};
use std::mem::discriminant;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::NamedTempFile;

use crate::config::FlockProcessConfig;

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
    pub pty_master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub parser: Arc<RwLock<vt100::Parser>>,
    pub status: ProcessRunningStatus,
}

impl Process {
    pub fn new(command: String) -> Result<Self> {
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
        writeln!(script, "{}", command)?;
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
            pty_master: Arc::new(Mutex::new(pair.master)),
            parser,
            status: ProcessRunningStatus::Stable,
        })
    }
}

#[derive(Clone)]
pub enum ProcessRunningStatus {
    Stable,
    Debouncing(RestartDebounceHandler),
    Restarting,
}

impl PartialEq for ProcessRunningStatus {
    fn eq(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }
}

#[derive(Clone)]
pub struct RestartDebounceHandler {
    started_at: Arc<RwLock<Instant>>,
}

impl RestartDebounceHandler {
    pub fn new(
        process_config: Arc<FlockProcessConfig>,
        status: Arc<RwLock<ProcessStatus>>,
    ) -> Self {
        let started_at = Arc::new(RwLock::new(Instant::now()));
        let s = Self { started_at };
        s.spawn_handler_thread(process_config, status);
        s
    }

    pub fn reset(&mut self) {
        if let Ok(mut started_at) = self.started_at.write() {
            *started_at = Instant::now();
        }
    }

    fn spawn_handler_thread(
        &self,
        process_config: Arc<FlockProcessConfig>,
        status: Arc<RwLock<ProcessStatus>>,
    ) {
        let duration = process_config.watch.debounce_duration();
        fn is_restartable(status: &ProcessStatus) -> bool {
            if let ProcessStatus::Running(process) = status {
                if let ProcessRunningStatus::Debouncing(_) = process.status {
                    return true;
                }
            }
            false
        }

        let handler = move || {
            let restartable = if let Ok(s) = status.read() {
                is_restartable(&*s)
            } else {
                false
            };

            if restartable {
                if let Ok(mut s) = status.write() {
                    if is_restartable(&*s) {
                        if let ProcessStatus::Running(process) = &mut *s {
                            process.status = ProcessRunningStatus::Restarting;

                            let process_config = process_config.clone();
                            let child = process.child.clone();
                            let status = status.clone();
                            std::thread::spawn(move || {
                                let restart = move |status: Arc<RwLock<ProcessStatus>>| {
                                    if let Ok(mut s) = status.write() {
                                        *s = ProcessStatus::Running(
                                            Process::new(process_config.command.to_owned())
                                                .unwrap(),
                                        );
                                    }
                                };
                                // Get the process ID
                                let pid = {
                                    let child_lock = child.read().unwrap();
                                    match child_lock.process_id() {
                                        Some(pid) => pid,
                                        None => {
                                            // No PID, notify completion and exit
                                            restart(status);
                                            return;
                                        }
                                    }
                                };
                                let nix_pid = Pid::from_raw(pid as i32);

                                // Send SIGTERM
                                let _ = kill(nix_pid, Signal::SIGTERM);

                                // Wait for process to exit with timeout
                                let start = Instant::now();
                                loop {
                                    let exit_status = {
                                        let mut child_lock = child.write().unwrap();
                                        child_lock.try_wait()
                                    };

                                    match exit_status {
                                        Ok(Some(_)) => {
                                            // Process exited, notify completion
                                            let _ = restart(status);
                                            return;
                                        }
                                        Ok(None) => {
                                            // Still running, check timeout
                                            if start.elapsed() >= Duration::from_secs(5) {
                                                // Timeout exceeded, send SIGKILL
                                                let _ = kill(nix_pid, Signal::SIGKILL);
                                                // Wait a bit for SIGKILL to take effect
                                                std::thread::sleep(Duration::from_millis(100));
                                                let _ = child.write().unwrap().try_wait();
                                                // Notify completion after SIGKILL
                                                let _ = restart(status);
                                                return;
                                            }
                                            std::thread::sleep(Duration::from_millis(50));
                                        }
                                        Err(_) => {
                                            // Error checking, assume exited, notify completion
                                            let _ = restart(status);
                                            return;
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
        };

        let started_at = self.started_at.clone();
        thread::spawn(move || {
            loop {
                if let Ok(started_at) = started_at.read() {
                    if started_at.elapsed() >= duration {
                        handler();
                        break;
                    }
                }
            }
        });
    }
}
