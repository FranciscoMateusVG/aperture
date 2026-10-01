//! Local-only server; installation and service lifecycle are separate operations.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => aperture_lib::web_server::serve().await,
        [verb] if verb == "open" => aperture_lib::web_server::open().await,
        [verb] if verb == "status" => aperture_lib::web_server::status().await.map(|s| println!("{s}")),
        [verb] if verb == "stop" => aperture_lib::web_server::stop().await,
        _ => Err("expected no arguments, open, status or stop".into()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aperture-server: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
