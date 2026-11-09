# Project rules

- **Audio callback rule:** no allocation, deallocation, locks, syscalls other than `Instant::now()`, logging, I/O. Enforced by `assert_no_alloc` in tests and debug builds.
- **Timing rule:** every event frame is computed from an anchor with exact integer math (Task 3). Never `t += dt`, never wall-clock scheduling.
- Tests never touch the real audio device unless marked `#[ignore]`.
