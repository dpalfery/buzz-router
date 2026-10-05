//! The `buzz-router` binary. All behaviour lives in the library target so tests can reach it.

fn main() -> std::process::ExitCode {
    buzz_router::cli::main()
}
