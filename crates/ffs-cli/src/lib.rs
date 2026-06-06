//! `ffs` CLI library. The binary is a thin wrapper that parses argv via
//! clap and calls [`run`]. The library exists so tests can drive
//! subcommands programmatically without spawning a subprocess.

pub mod client;
pub mod commands;
pub mod url;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

pub use commands::{
    EXIT_CAPABILITY_DENIED, EXIT_GENERAL, EXIT_NOT_FOUND, EXIT_OK, EXIT_USAGE, Outcome,
};

#[derive(Debug, Clone, Parser)]
#[command(name = "ffs", version, about = "FFS — command-line client", long_about = None)]
pub struct Args {
    /// Path to the daemon's local socket / named pipe.
    #[arg(long, env = "FFS_SOCKET", global = true)]
    pub socket: Option<PathBuf>,

    /// Emit JSON output where applicable.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Print the content addressed by an ffs:// URL.
    Cat { url: String },
    /// List entries at an ffs:// URL.
    Ls { url: String },
    /// Fetch the raw atom envelope for an atom or entity URL.
    Get { url: String },
    /// Print the daemon's daily health summary.
    Health,
    /// Inspect a predicate spec.
    Predicate {
        #[command(subcommand)]
        command: PredicateCommand,
    },
    /// Federation administration.
    Federation {
        #[command(subcommand)]
        command: FederationCommand,
    },
    /// Owner identity management.
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum IdentityCommand {
    /// Print the owner's public-key multibase and the source it was
    /// loaded from. Reads the keychain directly — works without a
    /// running daemon. Use this to confirm the substrate's identity
    /// is stable across restarts before federating with a peer.
    Show,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PredicateCommand {
    /// Inspect a predicate by name.
    Inspect { name: String },
}

#[derive(Debug, Clone, Subcommand)]
pub enum FederationCommand {
    /// Peer-administration commands.
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PeerCommand {
    /// Add a federation peer.
    Add {
        endpoint: String,
        fingerprint: String,
    },
    /// List federation peers.
    List,
}

/// Run a parsed `Args`. Returns an `Outcome` that the binary translates
/// into stdout/stderr writes and a process exit code.
pub async fn run(args: Args) -> Outcome {
    let socket = args.socket.unwrap_or_else(client::default_socket_path);
    let socket_ref = socket.as_path();
    let json = args.json;
    match args.command {
        Command::Cat { url } => commands::cat(socket_ref, &url, json).await,
        Command::Ls { url } => commands::ls(socket_ref, &url, json).await,
        Command::Get { url } => commands::get(socket_ref, &url).await,
        Command::Health => commands::health(socket_ref, json).await,
        Command::Predicate {
            command: PredicateCommand::Inspect { name },
        } => commands::predicate_inspect(socket_ref, &name).await,
        Command::Federation {
            command:
                FederationCommand::Peer {
                    command:
                        PeerCommand::Add {
                            endpoint,
                            fingerprint,
                        },
                },
        } => commands::federation_peer_add(socket_ref, &endpoint, &fingerprint).await,
        Command::Federation {
            command:
                FederationCommand::Peer {
                    command: PeerCommand::List,
                },
        } => commands::federation_peer_list(socket_ref).await,
        Command::Identity {
            command: IdentityCommand::Show,
        } => commands::identity_show(json),
    }
}
