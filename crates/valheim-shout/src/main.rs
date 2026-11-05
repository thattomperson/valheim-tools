//! `valheim-shout`: join a Valheim crossplay server by join code and shout.
//!
//! ```text
//! valheim-shout party-check --libparty ./libparty.so
//! valheim-shout lookup --code 123456
//! valheim-shout shout --code 123456 --password hunter2 --message "hello" --libparty ./libparty.so
//! ```

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use playfab_party::{Endpoint, Event as PartyEvent, Party};
use valheim_net::session::{ClientConfig, ClientSession, Event, State};
use valheim_playfab::{Client as PlayFab, JoinTarget, VALHEIM_TITLE_ID};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load libparty.so and call PartyInitialize, then clean up.
    PartyCheck {
        #[arg(long, env = "LIBPARTY", default_value = "./libparty.so")]
        libparty: PathBuf,
    },
    /// Log in to PlayFab and resolve a join code, without touching Party.
    Lookup {
        #[command(flatten)]
        id: Identity,
        /// The server's six-digit join code.
        #[arg(long)]
        code: String,
    },
    /// Join the server and send one shout.
    Shout(ShoutArgs),
}

#[derive(Args, Clone)]
struct Identity {
    /// Name shown in chat.
    #[arg(long, default_value = "ValheimTools")]
    name: String,
    /// Platform user ID the server sees. Also used as the PlayFab custom ID.
    /// Defaults to `ValheimTools_<hash of name>`.
    #[arg(long)]
    platform_id: Option<String>,
}

impl Identity {
    fn platform_id(&self) -> String {
        self.platform_id.clone().unwrap_or_else(|| {
            // FNV-1a: stable across runs, so the same PlayFab account is reused.
            let mut h: u64 = 0xcbf29ce484222325;
            for b in self.name.bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
            format!("ValheimTools_{h:016x}")
        })
    }
}

#[derive(Args)]
struct ShoutArgs {
    #[command(flatten)]
    id: Identity,
    #[arg(long)]
    code: String,
    #[arg(long, env = "VALHEIM_PASSWORD")]
    password: Option<String>,
    #[arg(long)]
    message: String,
    #[arg(long, env = "LIBPARTY", default_value = "./libparty.so")]
    libparty: PathBuf,
    /// Seconds to stay connected after the shout, printing chat.
    #[arg(long, default_value_t = 3)]
    linger: u64,
    /// Give up if not connected within this many seconds.
    #[arg(long, default_value_t = 60)]
    timeout: u64,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::PartyCheck { libparty } => party_check(&libparty),
        Command::Lookup { id, code } => lookup(&id, &code),
        Command::Shout(args) => shout(args),
    }
}

fn party_check(lib: &std::path::Path) -> Result<()> {
    let party = Party::initialize(lib, VALHEIM_TITLE_ID)?;
    println!("PartyInitialize(\"{VALHEIM_TITLE_ID}\") succeeded");
    let events = party.poll()?;
    println!("first poll returned {} state change(s)", events.len());
    drop(party);
    println!("PartyCleanup done");
    Ok(())
}

fn login(id: &Identity) -> Result<PlayFab> {
    let mut pf = PlayFab::valheim()?;
    let platform_id = id.platform_id();
    let s = pf
        .login_with_custom_id(&platform_id)
        .context("PlayFab login")?;
    tracing::info!(platform_id, playfab_id = %s.playfab_id, entity = %s.entity.id, "logged in");
    Ok(pf)
}

fn lookup(id: &Identity, code: &str) -> Result<()> {
    let pf = login(id)?;
    let t = pf.resolve_join_code(code)?;
    print_target(&t);
    pf.leave_lobby(&t.lobby_id)?;
    Ok(())
}

fn print_target(t: &JoinTarget) {
    println!("lobby:            {}", t.lobby_id);
    println!("server entity:    {}", t.server_entity_id);
    println!(
        "world:            {}",
        t.lobby_data.get("WORLD").map(String::as_str).unwrap_or("?")
    );
    println!("password needed:  {}", t.password_protected);
    println!("invitation id:    {}", t.network.invitation_id);
    println!(
        "descriptor:       {} chars",
        t.network.serialized_descriptor.len()
    );
}

fn random_uid() -> i64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    // Valheim UIDs are positive.
    (h.finish() >> 1) as i64
}

fn shout(args: ShoutArgs) -> Result<()> {
    let pf = login(&args.id)?;
    let target = pf.resolve_join_code(&args.code)?;
    print_target(&target);
    if target.password_protected && args.password.is_none() {
        let _ = pf.leave_lobby(&target.lobby_id);
        bail!("server needs a password: pass --password or set VALHEIM_PASSWORD");
    }
    let result = run_party(&args, &pf, &target);
    if let Err(e) = pf.leave_lobby(&target.lobby_id) {
        tracing::warn!("LeaveLobby failed: {e}");
    }
    result
}

fn run_party(args: &ShoutArgs, pf: &PlayFab, target: &JoinTarget) -> Result<()> {
    let me = pf.session().expect("logged in");
    let party = Party::initialize(&args.libparty, VALHEIM_TITLE_ID)?;
    let user = party.create_local_user(&me.entity.id, &me.entity_token)?;
    let network = party.connect_to_network(&target.network.serialized_descriptor)?;
    tracing::info!("joining Party network");

    let platform_id = args.id.platform_id();
    let mut session = ClientSession::new(ClientConfig {
        platform_user_id: platform_id.clone(),
        player_name: args.id.name.clone(),
        playfab_id: me.playfab_id.clone(),
        password: args.password.clone(),
        uid: random_uid(),
    });

    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.timeout);
    let mut local: Option<Endpoint> = None;
    let mut server: Option<Endpoint> = None;
    let mut connected_at: Option<Instant> = None;
    let mut shouted_at: Option<Instant> = None;
    let mut next_scan = started;

    // Errors break out through the closure so the goodbye below always runs.
    #[allow(clippy::redundant_closure_call)]
    let outcome = (|| -> Result<()> {
        loop {
            let now = Instant::now();
            for ev in party.poll()? {
                match ev {
                    PartyEvent::ConnectToNetworkCompleted { result, .. } => {
                        if !result.ok() {
                            bail!("could not join the Party network: {result}");
                        }
                        tracing::info!("Party network joined, authenticating");
                        party.authenticate_local_user(
                            network,
                            user,
                            &target.network.invitation_id,
                        )?;
                        party.create_endpoint(network, user)?;
                    }
                    PartyEvent::AuthenticateLocalUserCompleted { result } => {
                        if !result.ok() {
                            bail!("Party authentication failed: {result}");
                        }
                        tracing::info!("authenticated to Party network");
                    }
                    PartyEvent::CreateEndpointCompleted { result, endpoint } => {
                        if !result.ok() {
                            bail!("could not create our endpoint: {result}");
                        }
                        tracing::info!("our endpoint is up");
                        local = Some(endpoint);
                    }
                    PartyEvent::EndpointCreated {
                        endpoint,
                        entity_id,
                        is_local,
                    } => {
                        tracing::debug!(?entity_id, is_local, "endpoint created");
                        if !is_local
                            && entity_id
                                .as_deref()
                                .is_some_and(|id| id.eq_ignore_ascii_case(&target.server_entity_id))
                        {
                            tracing::info!("found the server endpoint");
                            server = Some(endpoint);
                        }
                    }
                    PartyEvent::EndpointDestroyed { endpoint, reason }
                        if Some(endpoint) == server =>
                    {
                        bail!("server endpoint went away (reason {reason})");
                    }
                    PartyEvent::MessageReceived { sender, data } if Some(sender) == server => {
                        session.receive(&data, now)?;
                    }
                    PartyEvent::NetworkDestroyed {
                        reason,
                        error_detail,
                    } => {
                        bail!(
                            "Party network destroyed (reason {reason}, detail 0x{error_detail:X})"
                        );
                    }
                    PartyEvent::LocalUserKicked => bail!("kicked from the Party network"),
                    PartyEvent::RemoteDeviceCreated => tracing::info!("remote device created"),
                    PartyEvent::RemoteDeviceJoinedNetwork => {
                        tracing::info!("remote device joined the network")
                    }
                    PartyEvent::RemoteDeviceLeftNetwork {
                        reason,
                        error_detail,
                    } => tracing::warn!(reason, error_detail, "remote device left the network"),
                    PartyEvent::MessageReceived { data, .. } => {
                        tracing::debug!(len = data.len(), "message from a non-server endpoint")
                    }
                    other => tracing::debug!(?other, "party event"),
                }
            }

            // Fallback and diagnostics: look at every endpoint Party knows,
            // in case the server's never arrives as ENDPOINT_CREATED.
            if local.is_some() && server.is_none() && now >= next_scan {
                next_scan = now + Duration::from_secs(3);
                let devices = party.network_device_count(network).unwrap_or(0);
                let endpoints = party.network_endpoints(network)?;
                let seen: Vec<_> = endpoints
                    .iter()
                    .map(|(_, id, local)| {
                        format!(
                            "{}{}",
                            id.as_deref().unwrap_or("?"),
                            if *local { " (us)" } else { "" }
                        )
                    })
                    .collect();
                tracing::info!(
                    devices,
                    endpoints = ?seen,
                    want = target.server_entity_id,
                    "waiting for the server endpoint"
                );
                if let Some((ep, _, _)) = endpoints.iter().find(|(_, id, is_local)| {
                    !is_local
                        && id
                            .as_deref()
                            .is_some_and(|id| id.eq_ignore_ascii_case(&target.server_entity_id))
                }) {
                    tracing::info!("found the server endpoint by listing");
                    server = Some(*ep);
                }
            }

            if let (Some(_), Some(_), State::Idle) = (local, server, session.state()) {
                tracing::info!(platform_id, "starting Valheim handshake");
                session.start(now);
            }
            session.poll(now);

            while let Some(ev) = session.next_event() {
                match ev {
                    Event::Connected(info) => {
                        tracing::info!(
                            server = info.server_name,
                            world = info.world_name,
                            version = info.version,
                            "connected"
                        );
                        connected_at = Some(now);
                    }
                    Event::PlayerList(list) => {
                        let names: Vec<_> = list.iter().map(|p| p.name.as_str()).collect();
                        tracing::info!(?names, "player list");
                        if shouted_at.is_none()
                            && list.iter().any(|p| p.platform_user_id == platform_id)
                        {
                            session.shout(&args.message, now)?;
                            tracing::info!(message = args.message, "shout sent");
                            shouted_at = Some(now);
                        }
                    }
                    Event::Chat { message, .. } => {
                        println!("[{:?}] {}: {}", message.kind, message.name, message.text);
                    }
                    Event::Rejected(status) => bail!("server refused us: {status}"),
                    Event::PasswordRequired => bail!("server needs a password"),
                    Event::VersionMismatch {
                        version,
                        network_version,
                    } => {
                        bail!("server runs {version} (network {network_version}); we speak 1.0.21 (41)")
                    }
                    Event::Disconnected => bail!("server disconnected us"),
                }
            }

            // Shout anyway if the player list never showed us.
            if let (Some(at), None) = (connected_at, shouted_at) {
                if now >= at + Duration::from_secs(5) {
                    tracing::warn!("our ID never appeared in the player list; shouting anyway");
                    session.shout(&args.message, now)?;
                    shouted_at = Some(now);
                }
            }

            if let Some(at) = shouted_at {
                if session.all_acked() && now >= at + Duration::from_secs(args.linger) {
                    break Ok(());
                }
            } else if now >= deadline {
                break Err(anyhow::anyhow!(
                    "timed out after {}s (local endpoint: {}, server endpoint: {}, session: {:?})",
                    args.timeout,
                    local.is_some(),
                    server.is_some(),
                    session.state()
                ));
            }

            if let (Some(l), Some(s)) = (local, server) {
                for msg in session.take_outgoing() {
                    party.send(l, s, &msg)?;
                }
            }
            sleep(Duration::from_millis(10));
        }
    })();

    // Say goodbye so the slot frees at once, then give Party a moment.
    let now = Instant::now();
    session.disconnect(now);
    if let (Some(l), Some(s)) = (local, server) {
        for msg in session.take_outgoing() {
            let _ = party.send(l, s, &msg);
        }
    }
    let _ = party.leave_network(network);
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if party
            .poll()?
            .iter()
            .any(|e| matches!(e, PartyEvent::LeaveNetworkCompleted))
        {
            break;
        }
        sleep(Duration::from_millis(10));
    }
    outcome
}
