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
    use crate::config::ConfigFormat;
    use log::info;

    let cli = args::parse();

    match &cli.command {
        Some(Commands::Generate { config }) => {
            let pg_doorman_config = generate::generate_config(config)?;

            // Determine output format based on file extension (default to TOML)
            let format = config
                .output
                .as_ref()
                .map(|p| ConfigFormat::detect(p))
                .unwrap_or(ConfigFormat::Toml);

            let data = match format {
                ConfigFormat::Yaml => serde_yaml::to_string(&pg_doorman_config)?,
                ConfigFormat::Toml => toml::to_string_pretty(&pg_doorman_config)?,
            };

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
