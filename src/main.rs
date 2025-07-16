use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::{env, fs};

use anyhow::Result;
use btreemultimap::BTreeMultiMap;
use clap::{command, Parser};
use home::home_dir;
use log::{debug, info, trace, LevelFilter};
use once_cell::sync::Lazy;
use regex::Regex;
use simple_logger::SimpleLogger;

static BASH_TIMESTAMP_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new("^#[0-9]{8}[0-9]*$").unwrap());
static ZSH_LINE_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new("^: ([0-9]{8}[0-9]*):([0-9]*);(.*)$").unwrap());

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// Reduces history file size, by removing:
/// - duplicates
/// - certain common commands
/// -
///
/// - scrape the file of anything confidential (passwords, etc.)
pub struct Args {
    /// Only preserve commands greater than this length
    #[arg(short, long, default_value = "15")]
    pub min_length: u16,
    /// Logging level. Default: Info. Valid values: Off, Error, Warn, Info, Debug, Trace.
    #[arg(short, long, default_value = "info", global = true)]
    pub log: LevelFilter,
    /// Path to the history file to process [default is $HISTFILE if HISTFILE is
    /// exported as an environment variable, otherwise ~/.bash_history]
    #[arg(short, long)]
    pub input: Option<String>,
    /// Name of the output file.
    #[arg(short, long, default_value = "shrunk_history")]
    pub output: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    SimpleLogger::new().with_level(args.log).init().unwrap();

    let history_file = if let Some(input_arg) = args.input {
        Path::new(&input_arg).into()
    } else if let Ok(env_var) = env::var("HISTFILE") {
        Path::new(&env_var).into()
    } else {
        home_dir().unwrap().join(".bash_history")
    };

    // Slurp the whole file into a string.
    let contents = fs::read_to_string(history_file)?;
    let lines = contents.lines();
    let lines: Vec<&str> = lines.collect();
    // The map that stores the commands that we will write out to the reduced history file.
    let mut command_map: BTreeMultiMap<u32, String> = BTreeMultiMap::new();
    // This set is used to strip out duplicate commands from the history.
    let mut commands_seen: HashSet<String> = HashSet::new();
    // And let's keep track of the largest commands, too.
    let mut big_commands: BTreeMultiMap<usize, String> = BTreeMultiMap::new();
    let mut flagged_commands: HashSet<String> = HashSet::new();

    // What type of history file is it?
    let is_zsh = is_zsh_extended(&lines);
    
    if is_zsh {
        process_zsh_history(
            lines,
            &mut command_map,
            &mut commands_seen,
            &mut big_commands,
            &mut flagged_commands,
        )?;
    } else {
        process_bash_history(
            lines,
            &mut command_map,
            &mut commands_seen,
            &mut big_commands,
            &mut flagged_commands,
        )?;
    }

    post_process(&args.output, is_zsh, command_map, big_commands, flagged_commands)?;

    Ok(())
}

fn is_zsh_extended(lines: &Vec<&str>) -> bool {
    // zsh EXTENDED_HISTORY format:
    // : 1746142083:0;cargo build --workspace --profile release
    //
    // bash format:
    // # 1746142083
    // cargo build --workspace --profile release
    //
    // plain format:
    // cargo build --workspace --profile release
    //
    // With or without the #timestamp lines, we can process as a bash history file.
    // So we only care if it's zsh extended or not.

    for &line in lines {
        if ZSH_LINE_REGEX.is_match(line) {
            return true;
        }
    }

    false
}

fn process_zsh_history(
    lines: Vec<&str>,
    command_map: &mut BTreeMultiMap<u32, String>,
    commands_seen: &mut HashSet<String>,
    big_commands: &mut BTreeMultiMap<usize, String>,
    flagged_commands: &mut HashSet<String>,
) -> Result<()> {
    let mut iter = lines.into_iter();
    while let Some(line) = iter.next() {
        if ZSH_LINE_REGEX.is_match(line) {
            let captures = ZSH_LINE_REGEX.captures(line).unwrap();
            let timestamp = captures[1].parse::<u32>()?;
            // zsh (on my system at least) seems to ignore the execution time field; it is always 0.
            // let execution_time = captures[2].parse::<u32>()?;
            let mut command = captures[3].trim().to_string();
            
            // Multi-line commands have escaped newlines
            while command.ends_with('\\') {
                if let Some(continuation) = iter.next() {
                    command = format!("{command}\n{continuation}");
                } else {
                    break;
                }
            }
            
            add_command(timestamp, &(command + "\n"), command_map, commands_seen, big_commands, flagged_commands);
        }
    }
    
    Ok(())
}

fn process_bash_history(
    lines: Vec<&str>,
    command_map: &mut BTreeMultiMap<u32, String>,
    commands_seen: &mut HashSet<String>,
    big_commands: &mut BTreeMultiMap<usize, String>,
    flagged_commands: &mut HashSet<String>,
) -> Result<()> {
    let mut timestamp = 0u32;
    let mut command = String::new();

    for line in lines {
        if let Some(new_timestamp) = parse_timestamp(line) {
            // We've found the timestamp for the next command. So add the existing
            // command with the previous timestamp;

            add_command(
                timestamp,
                &command,
                command_map,
                commands_seen,
                big_commands,
                flagged_commands,
            );
            timestamp = new_timestamp;
            command = String::new();
        } else {
            command = format!("{command}{line}\n");
        }
    }

    // Because in the above loop we only add a command when we see the next command's timestamp, we
    // won't have added the final command. So do that now.
    add_command(timestamp, &command, command_map, commands_seen, big_commands, flagged_commands);
    
    Ok(())
}

fn add_command(
    timestamp: u32,
    command: &str,
    command_map: &mut BTreeMultiMap<u32, String>,
    commands_seen: &mut HashSet<String>,
    big_commands: &mut BTreeMultiMap<usize, String>,
    flagged_commands: &mut HashSet<String>,
) {
    if !command.is_empty() {
        let filtered_command = filter_command(command);

        if !commands_seen.contains(&filtered_command) && !should_exclude_cmd(&filtered_command) {
            commands_seen.insert(filtered_command.clone());

            flag_command(&filtered_command, flagged_commands);

            if filtered_command.len() >= 200 {
                big_commands.insert(filtered_command.len(), filtered_command.clone());
            }
            command_map.insert(timestamp, filtered_command);
        }
    }
}

fn parse_timestamp(line: &str) -> Option<u32> {
    if BASH_TIMESTAMP_REGEX.is_match(line) {
        Some(line[1..].parse().unwrap())
    } else {
        None
    }
}

fn post_process(output: &str, is_zsh: bool, mut command_map: BTreeMultiMap<u32, String>, mut big_commands: BTreeMultiMap<usize, String>, mut flagged_commands: HashSet<String>) -> Result<()> {
    let mut big_command_lengths = big_commands.keys().collect::<Vec<&usize>>();
    big_command_lengths.sort();
    for length in big_command_lengths {
        let commands = big_commands.get_vec(length).unwrap();
        trace!("{} Commands of length {length}", commands.len());
        for command in commands {
            trace!("{}", command.trim_end());
        }
    }

    if !flagged_commands.is_empty() {
        info!("+=======================+");
        info!("| {:3} FLAGGED COMMANDS  |", flagged_commands.len());
        info!("+=======================+");
        flagged_commands
            .iter()
            .for_each(|command| info!("{}", command.trim_end()));
    }

    let mut output = File::create(output)?;
    for (timestamp, commands) in command_map {
        for command in commands {
            if is_zsh {
                output.write_all(format!(": {timestamp}:0;").as_bytes())?;
            } else {
                output.write_all(format!("#{timestamp}\n").as_bytes())?;
            }
            // command already ends in newline
            output.write_all(command.as_bytes())?;
        }
    }
    Ok(())
}

// I looked for my most common commands via:
//    # Omit the timestamp lines in the history file
//    grep -v '^#' $HISTFILE |
//      # Use awk to count the first word of every line
//      awk '{count[$1]++} END {for (word in count) print count[word], word}' |
//      # Sort numerically, in reverse
//      sort -rn |
//      # Take the top 20
//      head -n 20
// Here are the 20 most common:
// 585 cd
// 485 git
// 478 l
// 391 vi
// 375 rm
// 371 sk
// 363 g
// 287 cat
// 263 mv
// 263 docker
// 243 curl
// 214 echo
// 209 cp
// 203 for
// 191 fexpr
// 178 cargo
// 147 skuba
// 134 grep
// 120 gi
// 107 pbpaste
// Total == 5607, so over half of the 10,000 commands in my history.
static EXCLUDE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
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
    ]
});

static REPLACEMENTS: Lazy<Vec<(Regex, &str)>> = Lazy::new(|| {
    vec![
        (Regex::new("Authorization: Bearer [^'\"]*").unwrap(), "Authorization: Bearer xxx"),
        (Regex::new("password=\"[^$][^ ]*").unwrap(), "password=XXX"),
        (Regex::new("password=[^$][^ ]*").unwrap(), "password=XXX"),
        (Regex::new("password: ?[^ ]*").unwrap(), "password: XXX"),
    ]
});

static PATTERNS_TO_FLAG: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new("password").unwrap(),
        Regex::new("ssh").unwrap(),
        Regex::new("secret").unwrap(),
        Regex::new("base64").unwrap(),
        Regex::new("jasypt").unwrap(),
    ]
});

fn should_exclude_cmd(command: &str) -> bool {
    EXCLUDE_PATTERNS.iter().any(|regex| {
        let is_match = regex.is_match(command);
        if is_match {
            debug!("Cmd matches {regex}: {}", command.trim_end())
        }
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
