use anyhow::{bail, Result};
use clap::Args as ClapArgs;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Forwarded args reserved for future remote tagger orchestration
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub fn run(_args: Args) -> Result<()> {
    bail!("remote-tagger orchestration is not implemented yet")
}
