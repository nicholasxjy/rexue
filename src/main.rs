use clap::Parser;
fn main() {
    if let Err(error) = rexue::cli::run(rexue::cli::Cli::parse()) {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
        {
            return;
        }
        eprintln!("rexue: {error:#}");
        std::process::exit(1);
    }
}
