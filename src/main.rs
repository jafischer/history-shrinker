use std::collections::{HashSet};
use std::{env, fs};
use std::fs::File;
use std::io::Write;
use std::path::{Path};
use btreemultimap::BTreeMultiMap;

use clap::{command, Parser};
use home::home_dir;
use log::{debug, info, LevelFilter, trace};
use once_cell::sync::Lazy;
use regex::Regex;
use simple_logger::SimpleLogger;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// Reduces history file size.
///
/// I have a very large history file, and as a developer one of the most useful commands is
/// the following function:
/// ```bash
/// hgrep () {
///     history | grep "$@"
/// }
/// ```
/// I use it probably more than I google for things. It's like googling your command history.
/// I set my HISTSIZE and HISTFILESIZE to very large values (20000), so if I know I ran
/// some esoteric command anytime within the last few years, I will be able to grep for it.
///
/// Recently, I moved all my bash startup files into a git repo (and I'm amazed it took me
/// so long to do so). I also moved my bash history file there (by setting HISTFILE).
/// So, before committing changes to the history file, I want to do the following:
/// - reduce the size of the file
/// - scrape the file of anything confidential (passwords, etc.)
///
/// The history file format is:
/// - a timestamp line, `#dddddddddd` (number of seconds since the Unix epoch).
/// - One or more lines containing the command.
pub struct Args {
    /// Only preserve commands greater than this length
    #[arg(short, long, default_value = "15")]
    pub min_length: u16,
    /// Logging level. Default: Info. Valid values: Off, Error, Warn, Info, Debug, Trace.
    #[arg(short, long, default_value = "info", global = true)]
    pub log: LevelFilter,
    /// Path to the history file to process. Default is $HISTFILE if HISTFILE is
    /// exported as an environment variable, otherwise ~/.bash_history.
    pub histfile: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    SimpleLogger::new()
        .with_level(args.log)
        .init()
        .unwrap();

    let histfile = if let Some(histfile_arg) = args.histfile {
        Path::new(&histfile_arg).into()
    } else if let Ok(histfile_env) = env::var("HISTFILE") {
        Path::new(&histfile_env).into()
    } else {
        home_dir().unwrap().join(".bash_history")
    };
    
    // Slurp the whole file into a string.
    let contents = fs::read_to_string(histfile)?;
    let lines = contents
        .lines();

    let mut timestamp = 0u32;
    let mut command = String::new();
    // The map that stores the commands that we will write out to the reduced history file.
    let mut command_map: BTreeMultiMap<u32, String> = BTreeMultiMap::new();
    // This set is used to strip out duplicate commands from the history.
    let mut commands_seen: HashSet<String> = HashSet::new();
    // And let's keep track of the largest commands, too.
    let mut big_commands: BTreeMultiMap<usize, String> = BTreeMultiMap::new();
    let mut flagged_commands: HashSet<String> = HashSet::new();

    for line in lines {
        if let Some(new_timestamp) = parse_timestamp(line) {
            // We've found the timestamp for the next command. So add the existing
            // command with the previous timestamp;

            if !command.is_empty() && !commands_seen.contains(&command) && !should_exclude_cmd(&command) {
                let command = filter_command(&command);
                commands_seen.insert(command.clone());
                
                flag_command(&command, &mut flagged_commands);

                if command.len() > 100 {
                    big_commands.insert(command.len(), command.clone());
                }
                command_map.insert(timestamp, command);
            }
            timestamp = new_timestamp;
            command = String::new();
        } else {
            command = format!("{command}{line}\n");
        }
    }

    let mut big_command_lengths = big_commands.keys().collect::<Vec<&usize>>();
    big_command_lengths.sort();
    for length in big_command_lengths {
        let commands = big_commands.get_vec(&length).unwrap();
        trace!("{} Commands of length {length}", commands.len());
        for command in commands {
            trace!("    {command}\n");
        }
    }
    
    if !flagged_commands.is_empty() {
        info!("+=======================+");
        info!("| {:3} FLAGGED COMMANDS  |", flagged_commands.len());
        info!("+=======================+");
        flagged_commands.iter().for_each(|command| info!("{}", command.trim_end()));
    }
    
    let mut output = File::create("shrunk_bash_history")?;
    for (timestamp, commands) in command_map {
        for command in commands {
            output.write_all(format!("#{}\n", timestamp).as_bytes())?;
            // command already ends in newline
            output.write_all(format!("{command}").as_bytes())?;
        }
    }

    Ok(())
}


static TIMESTAMP_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new("^#[0-9]{8}[0-9]*$").unwrap());

fn parse_timestamp(line: &str) -> Option<u32> {
    if TIMESTAMP_REGEX.is_match(line) {
        Some(line[1..].parse().unwrap())
    } else {
        None
    }
}

// I looked for my most common commands via:
//    grep -v '^#' /Users/jafischer/src/bashrc/history-mac |
//      awk '{count[$1]++} END {for (word in count) print count[word], word}' |
//      sort -n
// Here are the 20 most common:
// 107 pbpaste
// 120 gi
// 134 grep
// 147 skuba
// 178 cargo
// 191 fexpr
// 203 for
// 209 cp
// 214 echo
// 243 curl
// 263 docker
// 263 mv
// 287 cat
// 363 g
// 371 sk
// 375 rm
// 391 vi
// 478 l
// 485 git
// 585 cd
// Total == 5607, so over half of the 10,000 commands in my history.
static EXCLUDE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    // Common commands (which I don't need to save)
    Regex::new("^echo ").unwrap(),
    Regex::new("^en ").unwrap(),
    Regex::new("^cd ").unwrap(),
    Regex::new("^cd$").unwrap(),
    Regex::new("^ls ").unwrap(),
    Regex::new("^ls$").unwrap(),
    Regex::new("^l ").unwrap(),
    Regex::new("^l$").unwrap(),
    Regex::new("^la ").unwrap(),
    Regex::new("^la$").unwrap(),
    Regex::new("^lt ").unwrap(),
    Regex::new("^lt$").unwrap(),
    Regex::new("^vi ").unwrap(),
    Regex::new("^md ").unwrap(),
    Regex::new("^rd ").unwrap(),
    Regex::new("^mv ").unwrap(),
    Regex::new("^rm ").unwrap(),
    Regex::new("^cp ").unwrap(),
    Regex::new("^ij ").unwrap(),
    Regex::new("^rr ").unwrap(),
    Regex::new("^s ").unwrap(),
    Regex::new("^type ").unwrap(),
    Regex::new("^sk8s ").unwrap(),
    Regex::new("^history").unwrap(),
    Regex::new("^fexpr ").unwrap(),
    Regex::new("^git add").unwrap(),
    Regex::new("^git pull").unwrap(),
    Regex::new("^gpull").unwrap(),
    Regex::new("^gst").unwrap(),
    Regex::new("^git status").unwrap(),
    Regex::new("^git checkout").unwrap(),
    Regex::new("^git mv").unwrap(),
    Regex::new("^git rm").unwrap(),
    Regex::new("^git diff").unwrap(),
    Regex::new("^git checkout").unwrap(),
    // All sk8s commands (e.g. 8l, 8h, 8logs)
    Regex::new("^8").unwrap(),
    Regex::new("help").unwrap(),

    // Commands with potential secrets
    Regex::new("echo.*\\| *pbcopy").unwrap(),
    Regex::new("en .*\\| *pbcopy").unwrap(),
    Regex::new("echo.*\\| *clip.exe").unwrap(),
    Regex::new("en .*\\| *clip.exe").unwrap(),
    Regex::new("echo.*\\| *base64").unwrap(),
    Regex::new("en .*\\| *base64").unwrap(),
]);

static REPLACEMENTS: Lazy<Vec<(Regex, &str)>> = Lazy::new(|| vec![
    (Regex::new("Authorization: Bearer [^'\"]*").unwrap(), "Authorization: Bearer xxx"),
    (Regex::new("password=\"[^$][^ ]*").unwrap(), "password=XXX"),
    (Regex::new("password=[^$][^ ]*").unwrap(), "password=XXX"),
    (Regex::new("password: ?[^ ]*").unwrap(), "password: XXX"),
]);

static PATTERNS_TO_FLAG: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    Regex::new("password").unwrap(),
    Regex::new("ssh").unwrap(),
    Regex::new("secret").unwrap(),
    Regex::new("base64").unwrap(),
    Regex::new("jasypt").unwrap(),
]);


fn should_exclude_cmd(command: &str) -> bool {
    EXCLUDE_PATTERNS.iter().any(|regex|
        {
            let is_match = regex.is_match(command);
            if is_match { debug!("Cmd matches {regex}: {}", command.trim_end()) }
            is_match
        })
}

fn filter_command(command: &str) -> String {
    let mut filtered_command: String = command.into(); 
    for (regex, replacement) in REPLACEMENTS.iter() {
        if regex.is_match(&filtered_command) {
            debug!("Replacing {regex} with {replacement} in {command}");
            filtered_command = regex.replace(&filtered_command, *replacement).into();
            debug!("Result: {command}");
        }
    }

    filtered_command
}

fn flag_command(command: &str, flagged_commands: &mut HashSet<String>) {
    if let Some(regex) = PATTERNS_TO_FLAG.iter().find(|regex| regex.is_match(command)) {
        flagged_commands.insert(format!("Flagged for '{regex}': {command}"));
    }
}