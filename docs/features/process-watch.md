# Process Watch Feature

## Overview

This feature enables automatic process restart when file changes are detected in the current working directory. When a user launches processes for a flock, the application will monitor the filesystem for changes and automatically restart the affected processes. The feature uses graceful shutdown by sending SIGTERM first, waiting for a timeout, then sending SIGKILL if the process has not terminated.

Key capabilities:
- Monitor the entire current working directory for file changes
- Automatically restart processes when changes are detected
- Graceful shutdown with SIGTERM followed by SIGKILL after timeout

## Core Implementation Library/Framework/Tool

| Library/Framework/Tool | Purpose |
|------------------------|---------|
| notify 8.x | Cross-platform filesystem notification library for detecting file changes |
| nix 0.29.x | Safe Rust bindings to Unix APIs for sending signals (SIGTERM, SIGKILL) to processes |
| crossbeam-channel 0.5.x | Multi-producer multi-consumer channels for communication between watcher thread and main event loop |

## Feature Components

### File System Watcher

A background thread monitors the current working directory recursively for file changes. The watcher uses `notify` to detect filesystem events in real-time.

The watcher sends events through a channel to the main application event loop.

### Process Restart on File Change

When a file change event is received and processes are running for the currently selected flock, the application triggers a restart sequence for all processes in that flock immediately.

### Graceful Shutdown Mechanism

When restarting a process (either due to file change or manual trigger), the application performs graceful shutdown:

1. Send SIGTERM to the process
2. Wait for the process to exit (with a configurable timeout, default 5 seconds)
3. If the process has not exited after timeout, send SIGKILL
4. Clean up PTY resources
5. Launch the new process instance

This ensures processes have an opportunity to perform cleanup operations before being forcefully terminated.

### Configuration Schema Extension

The YAML configuration is extended to support enabling/disabling file watching per process:

```yaml
flocks:
  - display_name: dev
    processes:
      - display_name: web server
        command: npm run dev
        watch: true
      - display_name: database
        command: docker-compose up db
        watch: false
```

Configuration field:
- `watch`: Boolean value to enable/disable file watching for this process (optional, default: `false`)

Only processes with `watch: true` will be automatically restarted when file changes are detected. Processes without this field or with `watch: false` will not be affected by file system events, though they can still be manually restarted via the Enter key.

### Event Loop Integration

The main event loop is modified to poll multiple event sources:
- Terminal keyboard events (existing)
- File watcher channel events (new)

When a file change event arrives, the application:
1. Checks if the selected flock has running processes
2. For each running process, triggers an immediate restart

### UI Feedback

The process panel displays visual feedback when:
- A process is being terminated (graceful shutdown in progress)
- A process is being relaunched

This helps users understand the current state of each process during the restart cycle.

## Challenges and Considerations

### Cross-Platform Signal Handling

The `nix` crate only works on Unix-like systems. For Windows compatibility, an alternative approach using `windows-sys` or conditional compilation would be needed. For the initial implementation, Unix-only support is acceptable given the target developer audience.

Possible solution: Use conditional compilation with `#[cfg(unix)]` and `#[cfg(windows)]` to provide platform-specific implementations. The `portable-pty` crate already handles some cross-platform concerns that can be leveraged.

### PTY Process Group Termination

Processes spawned via PTY may create child processes (e.g., shell spawning the actual command). Sending SIGTERM to the parent may not terminate all children, leading to orphaned processes.

Possible solution: Use process groups and send signals to the entire group using `killpg()` instead of `kill()`. The PTY slave process typically becomes a session leader, so signals can be sent to the process group.

### File Watcher Resource Limits

Operating systems limit the number of file watches. For large projects, watching the entire directory tree may exceed these limits.

Possible solution: Document the limitation and provide guidance on increasing system limits. Consider future enhancement to support glob patterns for selective watching.

### Rapid File Changes

Without debouncing, rapid file changes (e.g., during `git checkout` or mass file operations) will trigger multiple restarts in quick succession. This could lead to:
- Processes constantly restarting and never stabilizing
- High system resource usage
- Poor user experience

Possible solutions:
- Track process state (running, terminating, starting) and ignore restart requests while already restarting
- Queue at most one pending restart per process
- Consider adding simple rate limiting (e.g., minimum time between restarts)
