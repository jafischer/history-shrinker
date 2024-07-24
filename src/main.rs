use std::collections::{BTreeMap, HashSet};
use std::{env, fs};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{command, Parser};
use glob::glob;
use home::home_dir;
use multimap::MultiMap;
use regex::Regex;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// Combines history from multiple history files into one.
/// 
/// I wrote this tool because I have a couple years' worth of history saved in backups called `.bash_history-YYMM`,
/// and wanted to combine them (because every now and then I have ended up losing my history and starting fresh).
/// 
/// The history file format is:
/// - a timestamp line, `#dddddddddd` (number of seconds since the Unix epoch).
/// - One or more lines containing the command.
/// 
/// This tool reads in each history file, and stores the commands in two collections:
/// 1. A map of timestamp to command.
/// 2. A set of commands.
/// 
/// To handle the case where multiple commands have the same timestamp (which will happen when you paste multiple lines
/// into the terminal), I add a count suffix to the timestamp, e.g. `dddddddddd-01`, `dddddddddd-02`, etc.
/// 
/// And to handle the overlap between the history files, I use the set of commands to avoid storing the same command twice.
/// 
/// Once all the files have been processed, the resulting combined commands are saved to `.combined_bash_history`. 
pub struct Args {
    /// Omit commands less than this length
    #[arg(short, long, default_value = "10")]
    pub min_length: u16,
    /// Regex patterns of commands to exclude.
    #[arg(short, long)]
    pub exclude: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    
    let histfile = if let Ok(histfile_env) = env::var("HISTFILE") {
        Path::new(&histfile_env).into()
    } else {
        home_dir().unwrap().join(".bash_history")
    };

    let mut output = File::create("shrunk_bash_history")?;
    // The map that stores the commands that we will write out to the reduced history file.
    let mut command_map = BTreeMap::new();
    // This set is used to strip out duplicate commands from the history.
    let mut commands = HashSet::new();
    // And let's keep track of the largest commands, too.
    let mut big_commands: MultiMap<usize, String> = MultiMap::new();

    let contents = fs::read_to_string(&histfile)?;
    let lines = contents
        .lines();
    let name = histfile.file_name().unwrap().to_str().unwrap();
    let mut timestamp: Option<String> = None;
    let mut command = String::new();
    let timestamp_regex = Regex::new("^#[0-9]{10}$").unwrap();
    let min_length = args.min_length as usize;

    for (line_num, line) in lines.enumerate() {
        if timestamp_regex.is_match(line) {
            if timestamp.is_some() {
                if command.len() >= min_length && !commands.contains(&command) {
                    commands.insert(command.clone());

                    if command.len() > 100 {
                        big_commands.insert(command.len(), command.clone());
                    }
                    command_map.insert(timestamp.clone().unwrap(), command);
                }
                command = String::new();
            } else if line_num != 0 {
                println!("{name}: Skipped {line_num} lines");
            }
            timestamp = Some(line.into());
        } else {
            // Because it seems that HISTFILESIZE refers to the number of lines, not the number of commands
            // (including the timestamps?), history files often start with a few lines that have no 
            // timestamp, so we'll skip them.
            if timestamp.is_some() {
                command = if command.is_empty() { line.into() } else { format!("{command}\n{line}") };
            }
        }
    }

    let mut big_command_lengths = big_commands.keys().collect::<Vec<&usize>>();
    big_command_lengths.sort();
    for length in big_command_lengths {
        println!("{length}");
    }

    for (timestamp, command) in command_map {
        output.write_all(format!("{}\n", timestamp).as_bytes())?;
        output.write_all(format!("{command}\n").as_bytes())?;
    }

    Ok(())
}
