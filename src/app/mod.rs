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
        Some(Commands::GenerateDocs {
            output_dir,
            all_languages,
            russian,
        }) => {
            let languages: Vec<bool> = if *all_languages {
                vec![false, true] // EN then RU
            } else {
                vec![*russian]
            };

            for russian in &languages {
                let docs = vec![
                    ("general.md", generate::docs::generate_general_doc(*russian)),
                    ("pool.md", generate::docs::generate_pool_doc(*russian)),
                    (
                        "prometheus.md",
                        generate::docs::generate_prometheus_doc(*russian),
                    ),
                ];

                if let Some(ref base_dir) = output_dir {
                    let dir = if *russian {
                        format!("{base_dir}/ru")
                    } else {
                        base_dir.clone()
                    };
                    std::fs::create_dir_all(&dir)?;
                    for (name, content) in &docs {
                        let path = format!("{dir}/{name}");
                        std::fs::write(&path, content)?;
                        info!("Written: {path}");
                    }
                } else {
                    for (name, content) in &docs {
                        let lang = if *russian { "RU" } else { "EN" };
                        println!("--- {name} ({lang}) ---");
                        println!("{content}");
                    }
                }
            }
            std::process::exit(0);
        }
        None => (),
    }

    Ok(cli)
}
