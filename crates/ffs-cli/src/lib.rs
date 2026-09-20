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
    Health {
        /// Print the latest morning briefing instead (task_41).
        #[arg(long)]
        briefing: bool,
    },
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
    /// The courier: deterministic mailbox and feed intake (task_40).
    Courier {
        #[command(subcommand)]
        command: CourierCommand,
    },
    /// Capability grants: who may read, write, or auto-file (ADR-029).
    Capability {
        #[command(subcommand)]
        command: CapabilityCommand,
    },
    /// Attest that a fact still holds (ADR-034): a confirmation about
    /// one atom, signed by the owner key.
    Attest {
        /// The atom: an ffs://<graph>/atom/<hash> url or a bare hash.
        subject: String,
        /// re_read_same_source | independent_source | primary_source | owner_knowledge | contradicted_by
        #[arg(long)]
        basis: String,
        /// What you checked (a url, "person:<you>", a filing).
        #[arg(long)]
        source: Option<String>,
        /// The date the fact held (YYYY-MM-DD); defaults to today.
        #[arg(long = "as-of")]
        as_of: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum CapabilityCommand {
    /// Author an owner-signed grant. Example (the guide's default):
    /// `ffs capability grant --action accept --grantee mcp:agent/courier
    /// --predicates source.article,event.business,org.company,person.generic
    /// --max-per-day 50`
    Grant {
        /// read | write | supersede | accept | erase | classify | federate
        #[arg(long)]
        action: String,
        /// An Ed25519 multibase key or an agent identity such as mcp:agent/courier
        #[arg(long)]
        grantee: String,
        /// Comma-separated predicate names the grant covers
        #[arg(long, value_delimiter = ',')]
        predicates: Vec<String>,
        /// Comma-separated classification tiers (default: any)
        #[arg(long, value_delimiter = ',')]
        classifications: Vec<String>,
        /// Daily cap on auto-filed atoms; required for --action accept (50 is the guide's default)
        #[arg(long, conflicts_with = "unlimited")]
        max_per_day: Option<u32>,
        /// No daily cap (accept grants must say so explicitly)
        #[arg(long)]
        unlimited: bool,
        /// Expiry (ISO 8601 UTC)
        #[arg(long)]
        valid_to: Option<String>,
    },
    /// Active grants with their cap and today's usage.
    List,
    /// Revoke a grant (a superseding capability with no actions).
    Revoke { grant_hash: String },
}

#[derive(Debug, Clone, Subcommand)]
pub enum CourierCommand {
    /// Run one courier tick now (the daemon scheduler is task_41).
    Run {
        /// Perform every step but write files only to the dry-run
        /// scratch directory; nothing is submitted or marked seen.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the courier's last-run counters.
    Status,
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
        Command::Health { briefing: false } => commands::health(socket_ref, json).await,
        Command::Health { briefing: true } => commands::health_briefing(socket_ref, json).await,
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
        Command::Courier {
            command: CourierCommand::Run { dry_run },
        } => commands::courier_run(socket_ref, dry_run, json).await,
        Command::Courier {
            command: CourierCommand::Status,
        } => commands::courier_status(socket_ref, json).await,
        Command::Capability {
            command:
                CapabilityCommand::Grant {
                    action,
                    grantee,
                    predicates,
                    classifications,
                    max_per_day,
                    unlimited,
                    valid_to,
                },
        } => {
            commands::capability_grant(
                socket_ref,
                commands::GrantArgs {
                    action,
                    grantee,
                    predicates,
                    classifications,
                    max_per_day,
                    unlimited,
                    valid_to,
                },
                json,
            )
            .await
        }
        Command::Capability {
            command: CapabilityCommand::List,
        } => commands::capability_list(socket_ref, json).await,
        Command::Capability {
            command: CapabilityCommand::Revoke { grant_hash },
        } => commands::capability_revoke(socket_ref, &grant_hash, json).await,
        Command::Attest {
            subject,
            basis,
            source,
            as_of,
            note,
        } => commands::attest(socket_ref, &subject, &basis, source, as_of, note, json).await,
    }
}
