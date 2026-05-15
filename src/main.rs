use eyre::Result;
use octos_tui::{cli::Cli, event_loop};

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse()?;
    event_loop::run(cli)
}
