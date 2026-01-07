use log::error;
use std::io::{self, IsTerminal, Write};

use pg_doorman::config::{get_config, Config};
use tokio::runtime::Builder;

use pg_doorman::cmd_args::Args;

pub fn init_config(args: &Args) -> Result<Config, Box<dyn std::error::Error>> {
    // Создаём временный runtime, чтобы один раз асинхронно распарсить конфиг
    // (и корректно вывести ошибку до полноценной инициализации рантайма/логгера).
    {
        let runtime = Builder::new_multi_thread().worker_threads(1).build()?;
        runtime.block_on(async {
            match pg_doorman::config::parse(args.config_file.as_str()).await {
                Ok(_) => (),
                Err(err) => {
                    let stdin = io::stdin();
                    if stdin.is_terminal() {
                        eprintln!("Config parse error: {err}");
                        io::stdout().flush().unwrap();
                    } else {
                        error!("Config parse error: {err:?}");
                    }
                    std::process::exit(exitcode::CONFIG);
                }
            };
        });
    }

    Ok(get_config())
}
