// The app's C entry point: hands control to the Rust app (crates/ios),
// which runs UIKit's main loop and never returns.
_Noreturn void pm_ios_main(void);

int main(void) { pm_ios_main(); }
