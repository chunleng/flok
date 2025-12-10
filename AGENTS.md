# Flok

This is a Rust TUI program written with the ratatui framework. The program is
written to help developer setup development environment quickly with a single
command. In turn, the program will attempt to read from the config file and load
the flocks that are available. (Flocks in this case are groups of processes
useful to the project the developer wants to use it in, example in
@tests/assets/flok.json)

## Common Command

- Run check: `cargo check`
- Format after editing: `rustfmt --edition 2024 <file_name>`

## Note when changing code

Please check that the code can compile and is formatted after editing
