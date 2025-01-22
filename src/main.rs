use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "voir",
    about = "CLI Utility to Peek on Files",
    author = "Esteban Borai <estebanborai@gmail.com> (https://github.com/EstebanBorai/voir)",
    next_line_help = true
)]
pub struct Cli {
    /// Input File
    path: PathBuf,
}

fn main() -> Result<()> {
    let args = Cli::parse();
    let contents = std::fs::read_to_string(&args.path)?;

    println!("{}", contents);
    Ok(())
}
