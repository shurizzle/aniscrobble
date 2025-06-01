use std::io::Write;

use anyhow::{Context, Result, bail};
use api::Api;
use clap::{Parser, Subcommand};
use database::{Database, User};

mod api;
#[cfg(not(windows))]
mod daemon;
mod database;

pub trait IsFatal {
    fn is_fatal(&self) -> bool;
}

#[derive(Debug, Parser)]
#[command(version, about, long_about)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Login {
        /// force login even if already logged in
        #[arg(short, long)]
        force: bool,
    },
    Sync,
    Scrobble {
        /// sync in background
        #[arg(short, long)]
        background: bool,
        /// do not sync
        #[arg(short, long)]
        local_only: bool,
        anilist_id: u64,
        episode: u64,
    },
}

#[inline(always)]
fn _main() -> Result<()> {
    let mut cli = Cli::parse();
    loop {
        match match cli.command {
            Commands::Login { force } => login(force),
            Commands::Sync => sync(None),
            Commands::Scrobble {
                background,
                local_only,
                anilist_id,
                episode,
            } => scrobble(anilist_id, episode, background, local_only),
        }? {
            Some(c) => cli = c,
            None => return Ok(()),
        }
    }
}

#[cfg(not(debug_assertions))]
fn main() {
    if let Err(err) = _main() {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}

#[cfg(debug_assertions)]
fn main() -> Result<()> {
    _main()
}

#[cfg(debug_assertions)]
fn show_error<E: std::fmt::Debug + std::fmt::Display>(err: E) {
    eprintln!("Error: {err:#?}");
}

#[cfg(not(debug_assertions))]
fn show_error<E: std::fmt::Debug + std::fmt::Display>(err: E) {
    eprintln!("Error: {err}");
}

fn sync(db: Option<Database>) -> Result<Option<Cli>> {
    let db = if let Some(db) = db {
        db
    } else {
        Database::new()?
    };
    let Some(user) = db.login()? else {
        bail!("login not found")
    };
    let api = Api::new();
    let mut sync = db.sync()?;

    while let Some(anime) = sync.next() {
        let anime = anime?;
        match api.get_progress(&user.token, user.id, anime.id()) {
            Ok(api::Anime { progress, episodes }) => {
                let episode = if anime.episode() > progress {
                    if let Err(err) =
                        api.set_progress(&user.token, anime.id(), anime.episode(), episodes)
                    {
                        show_error(err);
                        None
                    } else {
                        Some(anime.episode())
                    }
                } else {
                    Some(progress)
                };
                if let Some(episode) = episode {
                    match anime.update(episode) {
                        Ok(_) => (),
                        Err(err) if err.is_fatal() => {
                            return Err(err.into());
                        }
                        Err(err) => show_error(err),
                    }
                }
            }
            Err(err) => show_error(err),
        }
    }

    sync.commit()?;
    Ok(None)
}

fn scrobble(
    anilist_id: u64,
    episode: u64,
    background: bool,
    local_only: bool,
) -> Result<Option<Cli>> {
    {
        let db = Database::new()?;
        db.scrobble(anilist_id, episode)?;
        if local_only {
            return Ok(None);
        }
        if !background {
            return sync(Some(db));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x8000000;
        const DETACHED_PROCESS: u32 = 8;

        std::process::Command::new(std::env::current_exe().context("Cannot spawn sync task")?)
            .arg("sync")
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .context("Cannot spawn sync task")?;
        Ok(None)
    }

    #[cfg(not(windows))]
    {
        match unsafe { daemon::daemonize()? } {
            daemon::Whoami::Child => Ok(Some(Cli {
                command: Commands::Sync,
            })),

            daemon::Whoami::Parent(res) => {
                if res {
                    Ok(None)
                } else {
                    bail!("The process could not be cloned");
                }
            }
        }
    }
}

const TOKEN_URL: &str =
    "https://anilist.co/api/v2/oauth/authorize?client_id=7723&response_type=token";

fn login(force: bool) -> Result<Option<Cli>> {
    let db = Database::new()?;
    if force {
        db.delete_login()?;
    }

    if db.login()?.is_some() {
        eprintln!("Already logged in");
        return Ok(None);
    }

    if open::that(TOKEN_URL).is_err() {
        println!("Please open {TOKEN_URL} in your browser and paste the given token here.")
    } else {
        println!("Paste here the token from your browser or manually open {TOKEN_URL}.")
    }

    let mut token = String::new();
    let api = Api::new();
    loop {
        loop {
            token.clear();
            print!("token> ");
            std::io::stdout().flush()?;
            std::io::stdin().read_line(&mut token)?;
            if token.is_empty() {
                std::process::exit(1);
            }
            if token.strip_suffix('\n').is_some() {
                token.remove(token.len() - 1);
                if token.strip_suffix('\r').is_some() {
                    token.remove(token.len() - 1);
                }
            } else if token.strip_suffix("\n\r").is_some() {
                token.remove(token.len() - 1);
                token.remove(token.len() - 1);
            }
            if !token.is_empty() {
                break;
            }
        }
        match api.me(&token).context("invalid token") {
            Ok(id) => {
                db.set_login(User { token, id })?;
                return Ok(None);
            }
            Err(err) => show_error(err),
        }
    }
}
