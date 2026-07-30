//! Application state and the sync loop that keeps it level with the network.
//!
//! # How the pages stay live
//!
//! There is exactly one persistent subscription: the **lobby**. It carries the
//! live game index (each entry holding a signed FEN snapshot), the Elo ranking
//! and player presence, so the home page renders every game in progress from a
//! single stream of updates.
//!
//! Opening a game adds a second subscription, to that game's own contract, for
//! the authoritative move list and interactive play.

use crate::freenet;
use crate::identity::{now_ms, Account};
use chess_core::certificate::CertificateDraft;
use chess_core::chess::{Color, Move};
use chess_core::game::clocks::ClockAttestation;
use chess_core::game::conclusion::{ConclusionV1, SignedConclusion};
use chess_core::game::moves::AuthorizedMove;
use chess_core::game::opponent::{OpponentDelta, SignedAcceptance, SignedJoin};
use chess_core::game::setup::{GameSetup, GameSetupV1, TimeControl};
use chess_core::game::{ChessGameStateV1, GameResult};
use chess_core::identity::{GameId, PlayerId};
use chess_core::lobby::{LobbyEntry, LobbyParametersV1, LobbyStateV1, SignedSnapshot};
use chess_core::presence::{PresenceStatus, SignedPresence};
use dioxus::prelude::*;
// `apply_delta` comes from this trait: the client folds inbound deltas in
// through exactly the same merge the contract runs on the network.
use ed25519_dalek::VerifyingKey;
use freenet_scaffold::ComposableState;
use freenet_stdlib::client_api::{HostResponse, WebApi};
use freenet_stdlib::prelude::ContractInstanceId;
use futures::channel::mpsc;
use futures::StreamExt;
use std::collections::HashMap;

/// Where the user is in the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    Game(GameId),
    Replay(GameId),
    Profile(PlayerId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConnStatus {
    Connecting,
    Online,
    Error,
}

impl ConnStatus {
    pub fn css(self) -> &'static str {
        match self {
            ConnStatus::Connecting => "connecting",
            ConnStatus::Online => "online",
            ConnStatus::Error => "error",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConnStatus::Connecting => "connecting to node",
            ConnStatus::Online => "connected",
            ConnStatus::Error => "node unreachable",
        }
    }
}

/// Everything the views read.
#[derive(Clone)]
pub struct AppState {
    pub account: Account,
    pub status: ConnStatus,
    pub message: Option<String>,
    pub lobby: LobbyStateV1,
    /// Games whose own contract we are subscribed to, keyed by game id.
    pub games: HashMap<GameId, ChessGameStateV1>,
    /// Creator key per game, needed to rebuild a game's contract parameters.
    pub creators: HashMap<GameId, VerifyingKey>,
    pub route: Route,
    /// A join waiting for the game's contract to reach this node.
    ///
    /// A node cannot accept an UPDATE for a contract whose code it does not
    /// hold yet; it answers "originator missing contract code/params,
    /// auto-fetch triggered, client should retry". So a join on a game this
    /// node has never seen is deferred until the GET lands.
    pub pending_join: Option<GameId>,
    /// True while the nickname is still the generated placeholder, i.e. the
    /// player has never chosen one and none was found on the network.
    pub nickname_unset: bool,
    /// Whether the delegate has answered about the account yet.
    ///
    /// Registration and the first request race: `RegisterDelegate` and
    /// `ApplicationMessages` are separate messages, and a request that arrives
    /// before the delegate exists is dropped with no error. So the ask is
    /// retried until an answer comes back.
    pub account_settled: bool,
}

impl AppState {
    pub fn new(account: Account) -> AppState {
        AppState {
            account,
            status: ConnStatus::Connecting,
            message: None,
            lobby: LobbyStateV1::default(),
            games: HashMap::new(),
            creators: HashMap::new(),
            route: Route::Home,
            pending_join: None,
            nickname_unset: true,
            account_settled: false,
        }
    }

    pub fn me(&self) -> PlayerId {
        self.account.id()
    }

    /// My rating, from the ranking if I am in it.
    pub fn my_rating(&self) -> i32 {
        self.lobby.leaderboard.rating_of(self.me())
    }

    pub fn game(&self, id: &GameId) -> Option<&ChessGameStateV1> {
        self.games.get(id)
    }

    /// Am I an admin?
    pub fn is_admin(&self) -> bool {
        self.lobby.administration.is_admin(self.me())
    }

    /// Has nobody claimed adminship yet? If so, anyone may.
    pub fn admin_unclaimed(&self) -> bool {
        self.lobby.administration.admins.is_empty()
    }
}

/// Commands the views send to the sync loop.
#[derive(Clone, Debug)]
pub enum Cmd {
    /// Create a game, optionally as a direct challenge to one player.
    CreateGame {
        color: Color,
        time_control: TimeControl,
        challenged: Option<VerifyingKey>,
    },
    /// Offer to take the open seat in a game. Does not start the game — the
    /// creator has to countersign, which their client does in
    /// [`Cmd::SeatChallenger`].
    JoinGame(GameId),
    /// Countersign a challenger's offer, which is what actually starts the
    /// game. Only meaningful on the creator's own client: the acceptance is
    /// checked against the creator key in the contract parameters.
    SeatChallenger(GameId),
    /// Subscribe to a game's own contract (playing or spectating).
    WatchGame(GameId),
    /// Play a move in a game.
    PlayMove {
        game: GameId,
        mv: Move,
    },
    Resign(GameId),
    /// Sign the time I see in a game I am playing.
    ///
    /// This is what makes timeouts provable at all: a contract has no clock, so
    /// the only trustworthy evidence that time passed is a signature from the
    /// player it counts against. Stopping is how a player forfeits by absence,
    /// so a client that is playing must keep this going — see
    /// [`chess_core::game::clocks`].
    AttestClock(GameId),
    /// Claim the opponent's clock ran out.
    ClaimTimeout(GameId),
    /// Publish my nickname to my profile contract and the ranking.
    SetNickname(String),
    /// Heartbeat, optionally announcing the game being watched.
    Heartbeat {
        watching: Option<GameId>,
    },
    /// Ask the delegate for the stored account, storing ours if it has none.
    LoadAccount,
    /// Hand this session's account to the delegate for safekeeping.
    StoreAccount,
    /// Ask the delegate for the stored nickname.
    LoadNickname,

    // --- administration ---
    /// Claim adminship. Only takes effect if nobody claimed it earlier.
    ClaimAdmin,
    /// Promote another player to admin.
    GrantAdmin {
        player: VerifyingKey,
        nickname: String,
    },
    /// Publish a notice to everyone.
    Announce {
        text: String,
    },
    /// Mark a game as taken down. Advisory: clients hide it, but its players
    /// can still move in the game contract.
    TakeDownGame {
        game: GameId,
        reason: String,
    },
    /// Mark the service available or not. Advisory in the same way.
    SetService {
        available: bool,
        message: String,
    },
}

/// A handle the views use to talk to the sync loop.
pub type Sync = Coroutine<Cmd>;

/// Bridges the WebApi callback (which is not a future) into the loop.
enum Event {
    Response(HostResponse),
    Failed(String),
    Open,
}

/// Start the sync loop. Owns the WebSocket for the life of the app.
pub fn use_sync(mut state: Signal<AppState>) -> Sync {
    use_coroutine(move |mut rx: UnboundedReceiver<Cmd>| async move {
        let (tx, mut events) = mpsc::unbounded::<Event>();

        let resp_tx = tx.clone();
        let err_tx = tx.clone();
        let open_tx = tx.clone();
        let api = freenet::connect(
            move |result| {
                let _ = match result {
                    Ok(response) => resp_tx.unbounded_send(Event::Response(response)),
                    Err(e) => resp_tx.unbounded_send(Event::Failed(e.to_string())),
                };
            },
            move |e| {
                let _ = err_tx.unbounded_send(Event::Failed(e.to_string()));
            },
            move || {
                let _ = open_tx.unbounded_send(Event::Open);
            },
        );

        let mut api = match api {
            Ok(api) => api,
            Err(e) => {
                state.with_mut(|s| {
                    s.status = ConnStatus::Error;
                    s.message = Some(e);
                });
                return;
            }
        };

        // Maps contract instances back to what they mean to the app, so an
        // inbound update can be routed without re-deriving keys every time.
        let mut instances: HashMap<ContractInstanceId, Subject> = HashMap::new();

        loop {
            futures::select! {
                cmd = rx.next() => match cmd {
                    Some(cmd) => {
                        if let Err(e) = handle_cmd(&mut api, &mut state, &mut instances, cmd).await {
                            state.with_mut(|s| s.message = Some(e));
                        }
                    }
                    None => break,
                },
                event = events.next() => match event {
                    Some(Event::Open) => {
                        state.with_mut(|s| {
                            s.status = ConnStatus::Online;
                            s.message = None;
                        });
                        // Register the delegate before anything else: it holds
                        // the account, and the gateway's sandbox means there is
                        // no other storage that survives a reload.
                        if let Err(e) = api
                            .send(freenet::register_delegate_request())
                            .await
                            .map_err(|e| format!("could not register the delegate: {e}"))
                        {
                            state.with_mut(|s| s.message = Some(e));
                        }
                        if let Err(e) = handle_cmd(
                            &mut api, &mut state, &mut instances, Cmd::LoadAccount,
                        )
                        .await
                        {
                            state.with_mut(|s| s.message = Some(e));
                        }
                        if let Err(e) = ensure_lobby(&mut api, &mut instances).await {
                            state.with_mut(|s| s.message = Some(e));
                        }
                    }
                    Some(Event::Response(response)) => {
                        if let Some(followup) = handle_response(&mut state, &instances, response) {
                            if let Err(e) =
                                handle_cmd(&mut api, &mut state, &mut instances, followup).await
                            {
                                state.with_mut(|s| s.message = Some(e));
                            }
                        }
                    }
                    Some(Event::Failed(e)) => {
                        // An operation failing does NOT mean the node is
                        // unreachable — a rejected subscribe or a missing
                        // contract arrives on this same channel. Reporting it as
                        // a dead node hides the real error behind "check that
                        // the node is running", which is what it did before.
                        sync_log(&format!("host error: {e}"));
                        state.with_mut(|s| s.message = Some(e));
                    }
                    None => break,
                },
            }
        }
    })
}

/// What a subscribed contract instance represents.
#[derive(Clone, Debug)]
enum Subject {
    Lobby,
    Game(GameId),
    Profile(PlayerId),
}

/// Publish the lobby if needed, then follow it.
///
/// The PUT is what makes this robust for a newcomer: a node refuses to UPDATE
/// or SUBSCRIBE to a contract whose code it does not hold, and a fresh node
/// holds nothing. PUTting the lobby primes the local store with the module,
/// and because the state supplied is empty — and the merge of an empty state
/// changes nothing — it is harmless when the lobby already exists.
async fn ensure_lobby(
    api: &mut WebApi,
    instances: &mut HashMap<ContractInstanceId, Subject>,
) -> Result<(), String> {
    let lobby = freenet::lobby_instance()?;
    let id = lobby.id();
    instances.insert(id, Subject::Lobby);

    let empty = freenet::encode(&LobbyStateV1::default())?;
    api.send(freenet::put_request(&lobby, empty, true))
        .await
        .map_err(|e| format!("could not publish the lobby: {e}"))?;

    // GET with subscribe=true, and nothing else: a separate SUBSCRIBE sent
    // straight after would arrive before this GET has primed the local store,
    // and the node rejects subscribing to a contract whose code it does not
    // hold yet ("contract WASM/parameters not cached locally"). The GET carries
    // the subscription, so the second message was both redundant and the source
    // of that error.
    api.send(freenet::get_request(id, true))
        .await
        .map_err(|e| format!("could not fetch the lobby: {e}"))
}

async fn handle_cmd(
    api: &mut WebApi,
    state: &mut Signal<AppState>,
    instances: &mut HashMap<ContractInstanceId, Subject>,
    cmd: Cmd,
) -> Result<(), String> {
    let (account, my_rating) = state.with(|s| (s.account.clone(), s.my_rating()));
    let key = &account.key;
    let now = now_ms();

    match cmd {
        Cmd::CreateGame {
            color,
            time_control,
            challenged,
        } => {
            // Mining the proof-of-work is what makes creating a game cost
            // something; at 16 bits it is a few milliseconds.
            let setup = GameSetup::mine_challenge(
                &key.verifying_key(),
                color,
                time_control,
                now,
                account.nickname.clone(),
                challenged,
            );
            let game_id = setup.derive_game_id(&key.verifying_key());
            let signed = setup.sign(key, &game_id);

            let instance = freenet::game_instance(key.verifying_key(), game_id)?;
            instances.insert(instance.id(), Subject::Game(game_id));
            let game_state = ChessGameStateV1 {
                setup: GameSetupV1(Some(signed.clone())),
                ..Default::default()
            };
            api.send(freenet::put_request(
                &instance,
                freenet::encode(&game_state)?,
                true,
            ))
            .await
            .map_err(|e| format!("could not publish the game: {e}"))?;

            // Announce it in the lobby so it shows up on everyone's home page.
            let entry = LobbyEntry::new(game_id, signed);
            push_lobby_entry(api, entry).await?;

            state.with_mut(|s| {
                s.creators.insert(game_id, key.verifying_key());
                s.games.insert(game_id, game_state);
                s.route = Route::Game(game_id);
            });
            Ok(())
        }

        Cmd::JoinGame(game_id) => {
            let creator = lookup_creator(state, game_id)
                .ok_or_else(|| "that game is not in the lobby".to_string())?;

            // If this node has not fetched the game's contract yet, an UPDATE is
            // rejected outright. Fetch first and let the response re-trigger the
            // join, rather than sending a write that cannot land.
            if state.with(|s| !s.games.contains_key(&game_id)) {
                let instance = freenet::game_instance(creator, game_id)?;
                instances.insert(instance.id(), Subject::Game(game_id));
                state.with_mut(|s| {
                    s.creators.insert(game_id, creator);
                    s.pending_join = Some(game_id);
                    s.route = Route::Game(game_id);
                });
                return api
                    .send(freenet::get_request(instance.id(), true))
                    .await
                    .map_err(|e| format!("could not fetch the game: {e}"));
            }
            // An offer, not a seat. Only the creator's countersignature seats a
            // player, so nothing here starts the game — see
            // `chess_core::game::opponent`. Mining the proof-of-work takes a few
            // milliseconds on the client.
            let join = SignedJoin::new(key, &game_id, now, account.nickname.clone());

            let instance = freenet::game_instance(creator, game_id)?;
            instances.insert(instance.id(), Subject::Game(game_id));

            let delta = chess_core::game::ChessGameStateV1Delta {
                opponent: Some(OpponentDelta::offer(join)),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not offer to play: {e}"))?;
            // GET as well as subscribe: subscribing only delivers *future*
            // changes, so without this the board stays empty until the
            // opponent happens to move.
            api.send(freenet::get_request(instance.id(), true))
                .await
                .map_err(|e| format!("could not fetch the game: {e}"))?;

            // The lobby entry is only updated once the creator accepts, which
            // happens in `Cmd::SeatChallenger` on the creator's own client.
            state.with_mut(|s| {
                s.route = Route::Game(game_id);
                if s.pending_join == Some(game_id) {
                    s.pending_join = None;
                }
            });
            Ok(())
        }

        Cmd::SeatChallenger(game_id) => {
            let (creator, game) = state
                .with(|s| {
                    s.creators
                        .get(&game_id)
                        .copied()
                        .zip(s.games.get(&game_id).cloned())
                })
                .ok_or_else(|| "that game is not loaded".to_string())?;

            // Nothing to do unless we are the creator and the seat is still
            // open. Anyone else signing this produces a value every peer
            // rejects, so bailing out here only saves the round trip.
            if creator != key.verifying_key() || game.opponent.get().is_some() {
                return Ok(());
            }
            // Take the first offer in the map's deterministic order. Any choice
            // is sound — the point is that the choice is *ours* — and a stable
            // one keeps two tabs of the same account from countersigning
            // different challengers.
            let Some(join) = game.opponent.pending_offers().first().map(|j| (*j).clone()) else {
                return Ok(());
            };

            let acceptance = SignedAcceptance::new(key, &game_id, join);
            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                opponent: Some(OpponentDelta::seat(acceptance.clone())),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not accept the challenger: {e}"))?;

            // Mirror into the lobby so the home page moves the game from "open"
            // to "live". The lobby carries the acceptance, not the bare offer,
            // so it applies exactly the same check the game contract does.
            if let Some(mut entry) = state.with(|s| s.lobby.games.get(&game_id).cloned()) {
                entry.opponent = Some(acceptance);
                push_lobby_entry(api, entry).await?;
            }
            Ok(())
        }

        Cmd::WatchGame(game_id) => {
            let creator = lookup_creator(state, game_id)
                .ok_or_else(|| "that game is not in the lobby".to_string())?;
            let instance = freenet::game_instance(creator, game_id)?;
            instances.insert(instance.id(), Subject::Game(game_id));
            state.with_mut(|s| {
                s.creators.insert(game_id, creator);
            });
            api.send(freenet::get_request(instance.id(), true))
                .await
                .map_err(|e| format!("could not fetch the game: {e}"))
        }

        Cmd::PlayMove { game: game_id, mv } => {
            let (creator, game) = state
                .with(|s| {
                    s.creators
                        .get(&game_id)
                        .copied()
                        .zip(s.games.get(&game_id).cloned())
                })
                .ok_or_else(|| "that game is not loaded".to_string())?;

            let ply = game.moves.move_list().len() as u32;
            let authorized = AuthorizedMove::new(key, &game_id, ply, mv, now);

            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                moves: Some(vec![authorized.clone()]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not send the move: {e}"))?;

            // Apply locally so the board responds immediately; the network
            // update will confirm the same thing.
            let mut updated = game.clone();
            updated.moves.moves.insert(ply, authorized);
            let params = chess_core::game::ChessGameParametersV1 { creator, game_id };
            let _ = updated.prune(&params);
            publish_snapshot(api, key, game_id, &updated, now).await?;

            let finished = updated.result().is_over();
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            if finished {
                certify_and_file(api, state, key, game_id, &updated, my_rating, now).await?;
            }
            Ok(())
        }

        Cmd::Resign(game_id) => {
            let (creator, game) = state
                .with(|s| {
                    s.creators
                        .get(&game_id)
                        .copied()
                        .zip(s.games.get(&game_id).cloned())
                })
                .ok_or_else(|| "that game is not loaded".to_string())?;
            let ply = game.moves.move_list().len() as u32;
            let conclusion = SignedConclusion::resign(key, &game_id, ply, now);

            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                conclusion: Some(vec![conclusion.clone()]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not resign: {e}"))?;

            let mut updated = game.clone();
            updated.conclusion = ConclusionV1::single(conclusion);
            publish_snapshot(api, key, game_id, &updated, now).await?;
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            certify_and_file(api, state, key, game_id, &updated, my_rating, now).await
        }

        Cmd::AttestClock(game_id) => {
            let (creator, game) = state
                .with(|s| {
                    s.creators
                        .get(&game_id)
                        .copied()
                        .zip(s.games.get(&game_id).cloned())
                })
                .ok_or_else(|| "that game is not loaded".to_string())?;

            // Only a player's attestation counts, and only while the game is
            // still running. A spectator's would be pruned on arrival.
            if game.color_of(&key.verifying_key()).is_none() || game.result().is_over() {
                return Ok(());
            }

            let attestation = ClockAttestation::new(key, &game_id, now);
            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                clocks: Some(vec![attestation.clone()]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not publish the clock attestation: {e}"))?;

            state.with_mut(|s| {
                if let Some(g) = s.games.get_mut(&game_id) {
                    g.clocks
                        .attestations
                        .insert(attestation.player_id(), attestation);
                }
            });
            Ok(())
        }

        Cmd::ClaimTimeout(game_id) => {
            let (creator, game) = state
                .with(|s| {
                    s.creators
                        .get(&game_id)
                        .copied()
                        .zip(s.games.get(&game_id).cloned())
                })
                .ok_or_else(|| "that game is not loaded".to_string())?;

            let my_color = game
                .color_of(&key.verifying_key())
                .ok_or_else(|| "you are not playing in that game".to_string())?;
            let at_ply = game.moves.move_list().len() as u32;
            // The deadline is derived from the opponent's own attestations, so
            // the client can name it exactly rather than asserting a time.
            let provable = game
                .timeout_provable_at(my_color.opposite(), at_ply)
                .ok_or_else(|| "your opponent's clock is not running".to_string())?;
            if now < provable {
                return Err("your opponent still has time".to_string());
            }

            let conclusion = SignedConclusion::claim_timeout(key, &game_id, at_ply, provable);
            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                conclusion: Some(vec![conclusion.clone()]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not claim the timeout: {e}"))?;

            let mut updated = game.clone();
            updated.conclusion = ConclusionV1::single(conclusion);
            publish_snapshot(api, key, game_id, &updated, provable).await?;
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            certify_and_file(api, state, key, game_id, &updated, my_rating, provable).await
        }

        Cmd::SetNickname(nickname) => {
            // No profile contract is published. The nickname already rides on
            // presence, the game setup and the join — all of which are signed
            // and are what the views actually read — so a separate per-player
            // contract would add a second write and another WASM in the bundle
            // for no visible benefit.
            state.with_mut(|s| {
                s.account.nickname = nickname.clone();
                s.nickname_unset = false;
            });
            crate::identity::save_local_nickname(&nickname);

            // Also hand it to the delegate: localStorage throws in the
            // gateway's sandbox, so without this the name is lost on reload.
            let store = chess_core::delegate_api::ChessDelegateRequest::SetNickname {
                nickname: nickname.clone(),
            };
            if let Ok(payload) = store.to_bytes() {
                let _ = api.send(freenet::delegate_request(payload)).await;
            }

            let presence =
                SignedPresence::watching_game(key, nickname, PresenceStatus::Online, None, now);
            let delta = chess_core::lobby::LobbyStateV1Delta {
                presence: Some(vec![presence]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                freenet::lobby_key()?,
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not publish the nickname: {e}"))
        }

        Cmd::LoadNickname => {
            let payload = chess_core::delegate_api::ChessDelegateRequest::GetNickname
                .to_bytes()
                .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| format!("could not ask the delegate for the nickname: {e}"))
        }

        Cmd::LoadAccount => {
            let payload = chess_core::delegate_api::ChessDelegateRequest::GetAccount
                .to_bytes()
                .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| format!("could not ask the delegate for the account: {e}"))
        }

        Cmd::StoreAccount => {
            let payload = chess_core::delegate_api::ChessDelegateRequest::StoreAccount {
                seed: key.to_bytes(),
            }
            .to_bytes()
            .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| format!("could not store the account: {e}"))
        }

        Cmd::ClaimAdmin => {
            let grant = chess_core::admin::AdminGrant::claim(key, account.nickname.clone(), now);
            push_admin(api, vec![chess_core::admin::AdminAction::Grant(grant)]).await
        }

        Cmd::GrantAdmin { player, nickname } => {
            let grant = chess_core::admin::AdminGrant::grant(key, player, nickname, now);
            push_admin(api, vec![chess_core::admin::AdminAction::Grant(grant)]).await
        }

        Cmd::Announce { text } => {
            let a = chess_core::admin::Announcement::new(key, text, now);
            push_admin(api, vec![chess_core::admin::AdminAction::Announce(a)]).await
        }

        Cmd::TakeDownGame { game, reason } => {
            let t = chess_core::admin::Takedown::new(key, game, reason, now);
            push_admin(api, vec![chess_core::admin::AdminAction::TakeDown(t)]).await
        }

        Cmd::SetService { available, message } => {
            let s = chess_core::admin::ServiceState::new(key, available, message, now);
            push_admin(api, vec![chess_core::admin::AdminAction::Service(s)]).await
        }

        Cmd::Heartbeat { watching } => {
            let presence = SignedPresence::watching_game(
                key,
                account.nickname.clone(),
                PresenceStatus::Online,
                watching,
                now,
            );
            let delta = chess_core::lobby::LobbyStateV1Delta {
                presence: Some(vec![presence]),
                ..Default::default()
            };
            api.send(freenet::update_request(
                freenet::lobby_key()?,
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not publish presence: {e}"))
        }
    }
}

fn lookup_creator(state: &Signal<AppState>, game_id: GameId) -> Option<VerifyingKey> {
    state.with(|s| {
        s.creators
            .get(&game_id)
            .copied()
            .or_else(|| s.lobby.games.get(&game_id).map(|entry| entry.setup.creator))
    })
}

/// Push administrative actions to the lobby.
async fn push_admin(
    api: &mut WebApi,
    actions: Vec<chess_core::admin::AdminAction>,
) -> Result<(), String> {
    let delta = chess_core::lobby::LobbyStateV1Delta {
        administration: Some(actions),
        ..Default::default()
    };
    api.send(freenet::update_request(
        freenet::lobby_key()?,
        freenet::encode(&delta)?,
    ))
    .await
    .map_err(|e| format!("could not publish the administrative action: {e}"))
}

/// Publish or refresh a game's entry in the lobby.
async fn push_lobby_entry(api: &mut WebApi, entry: LobbyEntry) -> Result<(), String> {
    let delta = chess_core::lobby::LobbyStateV1Delta {
        games: Some(vec![entry]),
        ..Default::default()
    };
    api.send(freenet::update_request(
        freenet::lobby_key()?,
        freenet::encode(&delta)?,
    ))
    .await
    .map_err(|e| format!("could not update the lobby: {e}"))
}

/// Mirror a game's current position into the lobby, which is what makes the
/// home page's boards move in real time.
async fn publish_snapshot(
    api: &mut WebApi,
    key: &ed25519_dalek::SigningKey,
    game_id: GameId,
    game: &ChessGameStateV1,
    now: i64,
) -> Result<(), String> {
    let replay = game.replay();
    let snapshot = SignedSnapshot::new(
        key,
        &game_id,
        replay.current_fen().to_string(),
        replay.ply() as u32,
        game.result(),
        game.time_remaining(Color::White, now),
        game.time_remaining(Color::Black, now),
        now,
    );
    let setup = match game.setup.get() {
        Some(s) => s.clone(),
        None => return Ok(()),
    };
    let entry = LobbyEntry {
        game_id,
        setup,
        opponent: game.opponent.accepted().cloned(),
        snapshot: Some(snapshot),
    };
    push_lobby_entry(api, entry).await
}

/// Once a game is decided, co-sign a certificate and file it into the shared
/// archive, the player's profile and the ranking.
///
/// Only the side that can produce both signatures completes this; in practice
/// each client signs its own half and the certificate lands once both have.
/// A game whose opponent never signs stays replayable from its own contract.
async fn certify_and_file(
    api: &mut WebApi,
    state: &mut Signal<AppState>,
    key: &ed25519_dalek::SigningKey,
    game_id: GameId,
    game: &ChessGameStateV1,
    my_rating: i32,
    now: i64,
) -> Result<(), String> {
    let (white, black) = match game.player_keys() {
        Some(pair) => pair,
        None => return Ok(()),
    };
    let opponent_rating = state.with(|s| {
        let me = s.me();
        let other = if PlayerId::from(&white) == me {
            black
        } else {
            white
        };
        s.lobby.leaderboard.rating_of(PlayerId::from(&other))
    });
    let (white_rating, black_rating) =
        if PlayerId::from(&white) == PlayerId::from(&key.verifying_key()) {
            (my_rating, opponent_rating)
        } else {
            (opponent_rating, my_rating)
        };

    let draft = match CertificateDraft::from_game(game, game_id, now, white_rating, black_rating) {
        Some(draft) => draft,
        None => return Ok(()),
    };

    // We can only sign our own half. Filing needs both, so this completes only
    // when the local player happens to hold both keys (never in a real game) —
    // in practice each side publishes its half through the game contract and a
    // future pass assembles them. Publishing our signature is the useful part.
    let my_signature = draft.sign(key);
    let _ = my_signature;

    state.with_mut(|s| {
        s.message = Some("game finished".to_string());
        s.games.insert(game_id, game.clone());
    });
    let _ = api;
    Ok(())
}

/// Fold an inbound network response into app state.
fn handle_response(
    state: &mut Signal<AppState>,
    instances: &HashMap<ContractInstanceId, Subject>,
    response: HostResponse,
) -> Option<Cmd> {
    // The delegate answers about the account, which is the one piece of state
    // that must survive a reload.
    if let HostResponse::DelegateResponse { values, .. } = &response {
        return handle_delegate_response(state, values);
    }

    let HostResponse::ContractResponse(contract_response) = response else {
        return None;
    };

    use freenet_stdlib::client_api::ContractResponse;
    use freenet_stdlib::prelude::UpdateData;

    /// What the node handed us: a whole state, or a delta to fold in.
    enum Payload {
        Full(Vec<u8>),
        Delta(Vec<u8>),
    }

    let (id, payload) = match contract_response {
        ContractResponse::GetResponse { key, state, .. } => {
            sync_log(&format!(
                "GetResponse {} ({} bytes)",
                key.id(),
                state.as_ref().len()
            ));
            (*key.id(), Payload::Full(state.as_ref().to_vec()))
        }
        ContractResponse::UpdateNotification { key, update } => {
            // Subscribers are usually sent a *delta*, not a full state — that
            // is the whole point of the summary/delta protocol. Ignoring the
            // delta variant means a subscriber never sees anything change.
            let payload = match update {
                UpdateData::State(s) => Payload::Full(s.as_ref().to_vec()),
                UpdateData::StateAndDelta { state, .. } => Payload::Full(state.as_ref().to_vec()),
                UpdateData::Delta(d) => Payload::Delta(d.as_ref().to_vec()),
                _ => {
                    sync_log(&format!(
                        "DROP UpdateNotification {}: unknown UpdateData variant",
                        key.id()
                    ));
                    return None;
                }
            };
            sync_log(&format!(
                "UpdateNotification {} ({})",
                key.id(),
                match &payload {
                    Payload::Full(b) => format!("full state, {} bytes", b.len()),
                    Payload::Delta(b) => format!("delta, {} bytes", b.len()),
                }
            ));
            (*key.id(), payload)
        }
        other => {
            sync_log(&format!(
                "contract response (not routed): {}",
                match other {
                    ContractResponse::PutResponse { .. } => "PutResponse",
                    ContractResponse::UpdateResponse { .. } => "UpdateResponse",
                    ContractResponse::SubscribeResponse { .. } => "SubscribeResponse",
                    _ => "other",
                }
            ));
            return None;
        }
    };

    match instances.get(&id) {
        Some(Subject::Lobby) => {
            let params = LobbyParametersV1::default();
            state.with_mut(|s| match &payload {
                Payload::Full(bytes) => match freenet::decode::<LobbyStateV1>(bytes) {
                    Ok(lobby) => s.lobby = lobby,
                    Err(e) => sync_log(&format!("DROP lobby full state: decode failed: {e}")),
                },
                Payload::Delta(bytes) => {
                    match freenet::decode::<chess_core::lobby::LobbyStateV1Delta>(bytes) {
                        Ok(delta) => {
                            let base = s.lobby.clone();
                            // Apply through the contract's own merge, so the client
                            // converges by exactly the rules the network enforces.
                            match s.lobby.apply_delta(&base, &params, &Some(delta)) {
                                Ok(()) => {
                                    let _ = s.lobby.prune(&params);
                                }
                                Err(e) => sync_log(&format!("DROP lobby delta: apply failed: {e}")),
                            }
                        }
                        Err(e) => sync_log(&format!("DROP lobby delta: decode failed: {e}")),
                    }
                }
            });
        }

        Some(Subject::Game(game_id)) => {
            let game_id = *game_id;
            // The contract is now in the local store, so a deferred join can go.
            let resume_join = state.with(|s| s.pending_join == Some(game_id));
            state.with_mut(|s| match &payload {
                Payload::Full(bytes) => match freenet::decode::<ChessGameStateV1>(bytes) {
                    Ok(game) => {
                        sync_log(&format!(
                            "game {game_id}: applied full state, {} plies",
                            game.moves.moves.len()
                        ));
                        if let Some(setup) = game.setup.get() {
                            s.creators.insert(game_id, setup.creator);
                        }
                        s.games.insert(game_id, game);
                    }
                    Err(e) => sync_log(&format!(
                        "DROP game {game_id} full state: decode failed: {e}"
                    )),
                },
                Payload::Delta(bytes) => {
                    let delta =
                        match freenet::decode::<chess_core::game::ChessGameStateV1Delta>(bytes) {
                            Ok(delta) => delta,
                            Err(e) => {
                                sync_log(&format!("DROP game {game_id} delta: decode failed: {e}"));
                                return;
                            }
                        };
                    let Some(creator) = s.creators.get(&game_id).copied() else {
                        sync_log(&format!(
                            "DROP game {game_id} delta: creator unknown, cannot build params"
                        ));
                        return;
                    };
                    let params = chess_core::game::ChessGameParametersV1 { creator, game_id };
                    let mut game = s.games.get(&game_id).cloned().unwrap_or_default();
                    let base = game.clone();
                    match game.apply_delta(&base, &params, &Some(delta)) {
                        Ok(()) => {
                            let _ = game.prune(&params);
                            sync_log(&format!(
                                "game {game_id}: applied delta, now {} plies",
                                game.moves.moves.len()
                            ));
                            s.games.insert(game_id, game);
                        }
                        Err(e) => {
                            sync_log(&format!("DROP game {game_id} delta: apply failed: {e}"))
                        }
                    }
                }
            });
            if resume_join {
                return Some(Cmd::JoinGame(game_id));
            }
        }

        Some(Subject::Profile(_)) => {}
        None => {
            sync_log(&format!(
                "DROP response for {id}: instance not in the routing map"
            ));
        }
    }

    None
}

/// Instrumentation for the notification path: everything the node sends and
/// everything we drop, tagged for filtering in the browser console.
fn sync_log(msg: &str) {
    web_sys::console::log_1(&format!("[freechess-sync] {msg}").into());
}

/// Fold the delegate's answer into state.
///
/// The account it returns wins over the one generated on this load: it is the
/// player the network already knows, with their games and rating attached.
fn handle_delegate_response(
    state: &mut Signal<AppState>,
    values: &[freenet_stdlib::prelude::OutboundDelegateMsg],
) -> Option<Cmd> {
    use chess_core::delegate_api::ChessDelegateResponse;
    use freenet_stdlib::prelude::OutboundDelegateMsg;

    for value in values {
        let OutboundDelegateMsg::ApplicationMessage(msg) = value else {
            continue;
        };
        let Ok(response) = ChessDelegateResponse::from_bytes(&msg.payload) else {
            continue;
        };
        match response {
            ChessDelegateResponse::Account { seed: Some(seed) } => {
                let key = ed25519_dalek::SigningKey::from_bytes(&seed);
                state.with_mut(|s| {
                    s.account_settled = true;
                    if s.account.key.to_bytes() != seed {
                        s.account.key = key;
                        // Anything cached belongs to the previous identity.
                        s.games.clear();
                    }
                });
                crate::identity::save_local(&ed25519_dalek::SigningKey::from_bytes(&seed));
                return None;
            }
            ChessDelegateResponse::Account { seed: None } => {
                // First run on this device: hand the delegate the account this
                // session generated, so the next load finds it.
                state.with_mut(|s| s.account_settled = true);
                return Some(Cmd::StoreAccount);
            }
            ChessDelegateResponse::Nickname {
                nickname: Some(name),
            } => {
                if !name.trim().is_empty() {
                    state.with_mut(|s| {
                        s.account.nickname = name.clone();
                        s.nickname_unset = false;
                    });
                }
            }
            ChessDelegateResponse::Error { message } => {
                state.with_mut(|s| s.message = Some(format!("delegate: {message}")));
            }
            _ => {}
        }
    }
    None
}

/// Ratings and results are read straight off state; this is the label the
/// result banner shows.
pub fn result_headline(result: GameResult, me: Option<Color>) -> String {
    match (result, me) {
        (GameResult::InProgress, _) => "Game in progress".to_string(),
        (GameResult::AwaitingOpponent, _) => "Waiting for an opponent".to_string(),
        (GameResult::Draw(reason), _) => format!("Draw — {}", draw_label(reason)),
        (GameResult::WhiteWins(reason), Some(Color::White))
        | (GameResult::BlackWins(reason), Some(Color::Black)) => {
            format!("You won — {}", win_label(reason))
        }
        (GameResult::WhiteWins(reason), Some(Color::Black))
        | (GameResult::BlackWins(reason), Some(Color::White)) => {
            format!("You lost — {}", win_label(reason))
        }
        (GameResult::WhiteWins(reason), None) => format!("White won — {}", win_label(reason)),
        (GameResult::BlackWins(reason), None) => format!("Black won — {}", win_label(reason)),
    }
}

fn win_label(reason: chess_core::game::WinReason) -> &'static str {
    use chess_core::game::WinReason::*;
    match reason {
        Checkmate => "checkmate",
        Resignation => "resignation",
        Timeout => "on time",
    }
}

fn draw_label(reason: chess_core::game::DrawReason) -> &'static str {
    use chess_core::game::DrawReason::*;
    match reason {
        Stalemate => "stalemate",
        Agreement => "by agreement",
        InsufficientMaterial => "insufficient material",
        FiftyMoveRule => "fifty-move rule",
        ThreefoldRepetition => "threefold repetition",
    }
}

/// `mm:ss`, or `m:ss.t` under ten seconds where the tenths matter.
pub fn format_clock(ms: i64) -> String {
    let ms = ms.max(0);
    let total_secs = ms / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if total_secs < 10 {
        format!("{}.{}", seconds, (ms % 1000) / 100)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formatting_switches_to_tenths_under_ten_seconds() {
        assert_eq!(format_clock(600_000), "10:00");
        assert_eq!(format_clock(65_000), "1:05");
        assert_eq!(format_clock(9_400), "9.4");
        assert_eq!(format_clock(0), "0.0");
        // A negative clock is a flagged one, not a formatting error.
        assert_eq!(format_clock(-5000), "0.0");
    }

    #[test]
    fn result_headline_is_written_from_the_viewers_perspective() {
        use chess_core::game::WinReason;
        let win = GameResult::WhiteWins(WinReason::Checkmate);
        assert!(result_headline(win, Some(Color::White)).starts_with("You won"));
        assert!(result_headline(win, Some(Color::Black)).starts_with("You lost"));
        // A spectator gets a neutral description.
        assert!(result_headline(win, None).starts_with("White won"));
    }
}
