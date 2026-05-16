use crate::config::{get_config, Config};
use tokio::runtime::Builder;

use crate::app::args::Args;

pub fn init_config(args: &Args) -> Result<Config, Box<dyn std::error::Error>> {
    // Создаём временный runtime, чтобы один раз асинхронно распарсить конфиг
    // (и корректно вывести ошибку до полноценной инициализации рантайма/логгера).
    {
        let runtime = Builder::new_multi_thread().worker_threads(1).build()?;
        runtime.block_on(async {
            match crate::config::parse(args.config_file.as_str()).await {
                Ok(_) => (),
                Err(err) => {
                    // Always write to stderr — the logger has not been
                    // initialized yet, so `log::error!` is swallowed on
                    // non-terminal stdin (CI, supervisor).
                    eprintln!("Config parse error: {err}");
                    std::process::exit(exitcode::CONFIG);
                }
            };
        });
    }

    Ok(get_config())
}
