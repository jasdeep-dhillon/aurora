//! Spike: host Valve's own `linux64/steamclient.so` as the global Steam
//! instance, with no steamui.so and no steamwebhelper.
//!
//! Nothing of Valve's is patched or bundled: the library is the user's own,
//! loaded as-is, and its exported `CreateInterface` gives the engine.
//!
//! `steamhost probe <steam-root>`        become the global instance and name
//!                                       the interfaces the getters return
//! `steamhost client-test <steam-root>`  connect to it like a game would
//! `steamhost login <steam-root>`        sign in (account, refresh token and
//!                                       SteamID on stdin), then serve games

#[cfg(target_os = "linux")]
mod engine;

/// Valve's own switches, read from /proc/self/cmdline: no assert minidumps
/// (which Breakpad would otherwise upload to crash.steampowered.com) and no
/// crash dialog or monitor.
#[cfg(target_os = "linux")]
const QUIET_FLAGS: [&str; 3] = ["-noassert", "-nocrashdialog", "-nocrashmonitor"];

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == QUIET_FLAGS[0]) {
        // Re-exec so the flags really are on our command line.
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("/proc/self/exe").args(&args[1..]).args(QUIET_FLAGS).exec();
        eprintln!("steamhost: re-exec failed: {err}");
        std::process::exit(1);
    }
    let (Some(cmd), Some(root)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: steamhost probe|client-test <steam-root>");
        std::process::exit(2);
    };
    let root = std::path::Path::new(root);
    let code = match cmd.as_str() {
        "probe" => engine::probe(root),
        "client-test" => engine::client_test(root),
        "login" => engine::login(root),
        "serve" => engine::serve(root),
        _ => Err(format!("unknown command {cmd}")),
    };
    let status = match code {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("steamhost: {e}");
            1
        }
    };
    // Valve's engine threads are still running: normal exit runs its static
    // destructors under them and segfaults. Leave without them.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    // SAFETY: _exit just ends the process.
    unsafe { libc::_exit(status) }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("steamhost only runs on Linux");
    std::process::exit(1);
}
