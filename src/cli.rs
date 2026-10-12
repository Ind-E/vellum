use std::path::PathBuf;

use color::DynamicColor;

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Option<Command>,

    /// Use this config file
    #[arg(long, conflicts_with = "no_config")]
    pub(super) config: Option<PathBuf>,

    /// Ignore config files
    #[arg(long)]
    pub(super) no_config: bool,
}

#[derive(clap::Subcommand)]
pub(crate) enum Command {
    /// Toggle drawing mode
    Toggle,
    /// Activate drawing mode
    Activate,
    /// Deactivate drawing mode
    Deactivate,
    /// Clear annotations
    Clear,
    /// Clear annotations and deactivate drawing mode
    ClearAndDeactivate,
    /// Set the drawing color
    SetColor {
        /// CSS color, such as `red`, `#00ffff`, or `oklab(0.5 0.2 0)`
        ///
        /// Quote colors containing #, spaces, or parentheses.
        color: DynamicColor,
    },
    /// Report whether drawing mode is active
    IsActive,
    /// Report whether text is being edited
    IsTextEditing,
    /// Stop Vellum
    Exit,
}
