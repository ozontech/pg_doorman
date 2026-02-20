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

pub use args::{parse, Args, Commands, GenerateConfig, LogFormat, OutputFormat};

pub fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    use crate::config::ConfigFormat;
    use log::info;

    let cli = args::parse();

    match &cli.command {
        Some(Commands::Generate { config }) => {
            // Determine output format: --format flag > file extension > YAML default
            let format = if let Some(ref fmt) = config.format {
                match fmt {
                    OutputFormat::Yaml => ConfigFormat::Yaml,
                    OutputFormat::Toml => ConfigFormat::Toml,
                }
            } else if let Some(ref path) = config.output {
                ConfigFormat::detect(path)
            } else {
                ConfigFormat::Yaml
            };

            let russian = config.russian_comments;

            let data = if config.reference {
                // --reference: generate reference config with example data, no PG connection needed
                generate::annotated::generate_reference_config(format, russian)
            } else {
                // Connect to PG and generate config
                let pg_doorman_config = generate::generate_config(config)?;

                if config.no_comments {
                    // --no-comments: plain serde serialization without comments
                    match format {
                        ConfigFormat::Yaml => serde_yaml::to_string(&pg_doorman_config)?,
                        ConfigFormat::Toml => toml::to_string_pretty(&pg_doorman_config)?,
                    }
                } else {
                    // Default: annotated config with comments
                    generate::annotated::generate_annotated_config(
                        &pg_doorman_config,
                        format,
                        russian,
                    )
                }
            };

            if let Some(output_path) = &config.output {
                std::fs::write(output_path, &data)?;
                info!("Config written to file: {output_path}");
            } else {
                println!("{data}");
            }
            std::process::exit(0);
        }
        None => (),
    }

    Ok(cli)
}
