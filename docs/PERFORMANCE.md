# Performance and Resource Budget

The target development machine has limited memory and CPU. Performance is an acceptance criterion, not a late optimization.

## Design rules
- No mandatory cloud process, local model, or separate database service.
- Prefer SQLite for local durable state over a separate server.
- Start desktop automation and expensive scans only on demand.
- Avoid duplicate PTY/process managers; one owner per capability.
- Bound file reads, directory traversal, terminal output, and audit retention.
- Avoid aggressive polling; use OS notifications where appropriate and verify their cost.
- Keep MCP server builds/features separate from the desktop build when this reduces compile/link overhead.
- Use one Cargo build job on constrained hardware when parallel builds cause memory pressure.

## Measurements
Measure on the actual target system, recording versions and conditions:
- cold startup and time until UI is usable;
- idle RAM and CPU after settling;
- active terminal session RAM/CPU;
- filesystem listing and bounded file-read latency;
- process metrics polling overhead;
- desktop automation activation overhead;
- clean build duration and peak memory;
- packaged application size.

Do not invent numeric budgets before establishing a baseline. Set budgets from measured baseline plus explicit headroom. Repeat after adding a capability or major dependency.
