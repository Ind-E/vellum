use std::process::ExitCode;

use clap::Parser;

mod cli;
mod config;
mod draw;
mod event_loop;
mod ipc;
mod render;
mod state;
mod text;
mod tool;

use cli::{Cli, Command};

pub(crate) type Rgba = [f32; 4];
pub(crate) type OutputId = u32;

pub(crate) fn color_to_srgb(color: color::DynamicColor) -> Rgba {
    color.convert(color::ColorSpaceTag::Srgb).clip().components
}

fn main() -> ExitCode {
    match run() {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("vellum: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let arguments = Cli::parse();
    if let Some(subcommand) = &arguments.command {
        let truthy_value = match subcommand {
            Command::IsActive => ipc::query(Command::IsActive)?,
            Command::IsTextEditing => ipc::query(Command::IsTextEditing)?,
            _ => {
                ipc::send_command(subcommand)?;
                return Ok(ExitCode::SUCCESS);
            }
        };
        println!("{truthy_value}");
        return Ok(if truthy_value {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }
    let settings = config::Settings::load(arguments)?;
    event_loop::run(settings)?;
    Ok(ExitCode::SUCCESS)
}
