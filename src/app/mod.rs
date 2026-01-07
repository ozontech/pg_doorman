pub mod args;
pub mod config;
pub mod errors;
pub mod generate;
pub mod logger;
pub mod panic;
pub mod server;
pub mod tls;

pub use config::init_config;
pub use logger::init_logging;
pub use panic::install_panic_hook;
pub use server::run_server;

pub use args::{parse, Args, Commands, GenerateConfig, LogFormat};

pub fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    use log::info;

    let cli = args::parse();

    match &cli.command {
        Some(Commands::Generate { config }) => {
            let pg_doorman_config = generate::generate_config(config)?;
            let data = toml::to_string_pretty(&pg_doorman_config)?;
            if let Some(output_path) = &config.output {
                std::fs::write(output_path, &data)?;
                info!("Config written to file: {output_path}");
            } else {
                println!("{data}");
            }
            // Для `generate` сервер не запускаем.
            std::process::exit(0);
        }
        None => (),
    }

    Ok(cli)
}
