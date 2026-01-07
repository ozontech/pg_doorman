use log::info;

use pg_doorman::cmd_args::{self, Args, Commands};
use pg_doorman::generate::generate_config;

pub fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let cli = cmd_args::parse();

    match &cli.command {
        Some(Commands::Generate { config }) => {
            let pg_doorman_config = generate_config(config)?;
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
