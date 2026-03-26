---
inclusion: always
---
1. After making changes don't forget to do a build (incremental, only do clean if absolutely neccessary) and fix any compile time issues 
2. when building, look at the full build output to find and fix all errors and warnings (and not just the last 30 lines)
3. NEVER commit changes unless the user has EXPLICITLY told you to commit. Do not commit proactively, do not commit after completing a task, do not commit when asked to "do it" or "go ahead". The ONLY trigger for a commit is the user saying words like "commit", "please commit", or "commit this". Always wait for the user to test first.
4. The computer-control-mcp is a SEPARATE binary. If you change `src/bin/computer_control_mcp.rs`, you MUST rebuild it with `cargo build --bin computer-control-mcp` (kill running instances first if locked). `cargo tauri dev` and `cargo check` do NOT rebuild it.
