# Basic Feature

## Overview

This document describes the core feature of Flok: reading a configuration file and presenting a TUI interface that allows developers to execute groups of commands called "flocks." A flock represents a collection of processes that are useful for setting up a development environment, such as running Docker containers, starting development servers, or executing build watchers.

The feature enables developers to:
- Define multiple flocks in a YAML configuration file
- Browse available flocks through an intuitive TUI interface
- Start all processes within a selected flock in parallel with a single keypress
- Monitor real-time output from all running processes in split panes

## Core Implementation Library/Framework/Tool

| Library/Framework/Tool | Purpose |
|------------------------|---------|
| ratatui | TUI framework for rendering the interface, layouts, and widgets |
| crossterm | Terminal manipulation and keyboard event handling |
| serde | Serialization/deserialization framework for configuration parsing |
| serde_yaml | YAML format support for configuration file parsing |
| portable-pty | Cross-platform PTY (pseudo-terminal) support for process spawning |
| vt100 | Terminal escape sequence parsing for proper output rendering with colors |
| tui-widget-list | Scrollable list widget for flock selection interface |

## Feature Components

### Configuration Management

The configuration system reads and parses a YAML file that defines available flocks and their associated processes.

**Configuration File Location:**
- Fixed path: `./flok.yaml` in the current working directory
- No search in parent directories or home directory

**YAML Schema:**

```yaml
flocks:
  - display_name: <string>      # Human-readable name shown in the TUI
    processes:
      - display_name: <string>  # Human-readable name for the process pane title
        command: <string>       # Shell command to execute
      - display_name: <string>
        command: <string>
  - display_name: <string>
    processes:
      - display_name: <string>
        command: <string>
```

**Example Configuration:**

```yaml
flocks:
  - display_name: dev
    processes:
      - display_name: docker compose
        command: docker-compose -f ./docker-compose.yaml up
      - display_name: print to 100
        command: for i in `seq 1 100`; do echo $i; sleep 1; done
  - display_name: dev with compose only
    processes:
      - display_name: docker compose
        command: docker-compose -f ./docker-compose.yaml up
```

**Validation Requirements:**
- File must exist at `./flok.yaml`
- File must be valid YAML syntax
- Root element must contain a `flocks` array
- Each flock must have a `display_name` and `processes` array
- Each process must have a `display_name` and `command`

### Flock Selection Interface

A navigable list interface displayed on the left side of the terminal allowing users to browse and select available flocks.

**User Interaction:**
- Arrow keys (Up/Down) or Vim-style keys (j/k) for navigation
- Visual highlight on the currently selected flock (reversed colors)
- Enter key to start the selected flock

### Split-Pane Process Output Display

The main display area shows output from all processes in the currently selected flock, with each process having its own dedicated pane.

**Layout Behavior:**
- Occupies approximately 80% of the terminal width (right side)
- Vertically splits the area equally among all processes in the selected flock
- Each pane has a border with the process `display_name` as the title
- Panes automatically resize when terminal dimensions change

**Output Rendering:**
- VT100 escape sequences are parsed and rendered with proper formatting
- Supports ANSI colors (16-color palette)
- Supports text attributes: bold, italic, underline
- Output streams in real-time as processes produce output
- PTY dimensions match pane dimensions (minus borders) for proper line wrapping

### Flock Execution Engine

The execution engine handles spawning and managing all processes within a flock when the user initiates execution.

**Execution Model:**
- All processes in a flock start simultaneously (parallel execution)
- Each process runs in its own PTY for proper terminal emulation
- Commands execute through the user's login shell (from `$SHELL` environment variable, fallback to `sh`)
- Background threads read process output and feed it to VT100 parsers

**PTY Configuration:**
- Dynamically resized to match the display pane dimensions
- Full terminal emulation support for interactive commands

**Output Collection:**
- Dedicated reader thread per process
- 8KB read buffer for efficient I/O
- Output fed to VT100 parser for escape sequence processing
- Parser state shared between reader thread and render loop via thread-safe reference

### User Interface Flow

**Application Startup:**
1. Application launches and reads `./flok.yaml` from the current directory
2. Configuration is parsed and validated
3. TUI initializes with the terminal in raw mode
4. Flock list is displayed with the first flock selected
5. Empty process panes are shown for the selected flock

**Flock Navigation:**
1. User presses Up/Down arrows or j/k keys
2. Selection highlight moves to the adjacent flock
3. Process panes update to show the processes of the newly selected flock
4. If processes were running for the previously selected flock, they continue running in the background

**Flock Execution:**
1. User presses Enter on a selected flock
2. All processes in the flock spawn simultaneously
3. Each process gets its own PTY and reader thread
4. Output begins streaming to the respective panes
5. Panes display real-time output with colors and formatting

**Application Exit:**
1. User presses 'q' or Ctrl+C
2. Terminal is restored to normal mode
3. Application exits (running processes may be orphaned)

### Keyboard Controls

| Key | Action |
|-----|--------|
| Up / k | Move selection to previous flock |
| Down / j | Move selection to next flock |
| Enter | Start all processes in selected flock |
| q | Exit application |
| Ctrl+C | Exit application |

## Challenges and Considerations

### Terminal Size Handling

When the terminal is resized, the PTY dimensions for running processes must be updated to match the new pane sizes. This ensures proper line wrapping and prevents output corruption. The resize operation must be performed atomically on both the PTY master and the VT100 parser to maintain consistency.

### Process Output Synchronization

Multiple threads write to shared VT100 parser state while the main thread reads from it for rendering. Thread-safe primitives (RwLock) are required to prevent data races. The render loop should acquire read locks briefly to minimize contention with output reader threads.

### Shell Command Execution

Commands are executed through the user's login shell to ensure proper environment setup and command interpretation. The command string is written to a temporary script file and executed, which allows for complex shell constructs (pipes, loops, redirections) to work correctly.

### Color Support Limitations

The current implementation supports the basic 16-color ANSI palette. Consider extended 256-color or true-color output from processes will fall back to the default color, which may result in visual discrepancies for processes that use rich color output.
