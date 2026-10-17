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
use freenet_stdlib::prelude::{ContractInstanceId, DelegateKey};
use futures::channel::mpsc;
use futures::StreamExt;
use std::collections::HashMap;

/// Where the user is in the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    Practice,
    Game(GameId),
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
    /// Whether the retired delegates have already been asked this session, so
    /// a second empty answer does not restart the probe in a loop.
    pub legacy_probed: bool,
    pub legacy_account: Option<(DelegateKey, PlayerId)>,
    pub legacy_nickname_pending: bool,
    pub legacy_theme_source: Option<DelegateKey>,
    pub pending_import: Option<[u8; 32]>,
    pending_import_context: Option<Vec<u8>>,
    theme_probe: Option<ThemeProbe>,
    /// The account whose private appearance settings have finished loading.
    pub theme_player: Option<[u8; 32]>,
    pub stored_theme: Option<crate::theme::Theme>,
    pub pending_theme: Option<crate::theme::Theme>,
    pub theme_error: Option<String>,
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
            legacy_probed: false,
            legacy_account: None,
            legacy_nickname_pending: false,
            legacy_theme_source: None,
            pending_import: None,
            pending_import_context: None,
            theme_probe: None,
            theme_player: None,
            stored_theme: None,
            pending_theme: None,
            theme_error: None,
        }
    }

    pub fn me(&self) -> PlayerId {
        self.account.id()
    }

    /// Challenges still waiting on an answer from me.
    ///
    /// A challenge I have already accepted is NOT one of them, even though the
    /// game stays "open" until the creator countersigns my offer: taking the
    /// seat is a two-step handshake, and only the first step is mine. Counting
    /// it until the creator acts would leave the badge lit after the one action
    /// it was asking for — which is exactly what it looked like.
    pub fn pending_challenges(&self, now: i64) -> Vec<&chess_core::lobby::LobbyEntry> {
        let me = self.me();
        let my_key = self.account.key.verifying_key();
        self.lobby
            .games
            .challenges_for(me, now)
            .into_iter()
            .filter(|e| !self.lobby.administration.is_taken_down(&e.game_id))
            .filter(|e| {
                // Answered already if our offer is on the table, or if the seat
                // is filled and the lobby has simply not caught up yet.
                !self
                    .games
                    .get(&e.game_id)
                    .map(|g| g.opponent.has_offer_from(&my_key) || g.opponent.get().is_some())
                    .unwrap_or(false)
            })
            .collect()
    }

    /// My rating, from the ranking if I am in it.
    pub fn my_rating(&self) -> i32 {
        self.lobby.leaderboard.rating_of(self.me())
    }

    pub fn game(&self, id: &GameId) -> Option<&ChessGameStateV1> {
        self.games.get(id)
    }

    pub fn can_create_game(&self) -> bool {
        self.status == ConnStatus::Online
            && self.account_settled
            && self.lobby.administration.accepting_new_games()
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
    /// Re-publish our own offer for a seat we have not been given yet.
    ///
    /// The other half of the same repair: a lost delta in either direction
    /// leaves the two sides disagreeing about whether a game has started, and
    /// nothing else would ever retry.
    ResendOffer(GameId),
    /// Countersign a challenger's offer, which is what actually starts the
    /// game. Only meaningful on the creator's own client: the acceptance is
    /// checked against the creator key in the contract parameters.
    SeatChallenger(GameId),
    /// Refuse a direct challenge, so the challenger stops waiting on an answer
    /// that is never coming.
    DeclineChallenge(GameId),
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
    /// End a game whose opponent has run out of time.
    ///
    /// Sent on a timer, not by a button. Winning on time is not something one
    /// player does to another — it is what happens when a clock reaches zero —
    /// and offering it as a control invited pressing it the moment the contract
    /// would accept it, which is well before the clock is actually empty.
    ClaimTimeout(GameId),
    /// Take our part in certifying a finished game, and file it once both
    /// halves are in.
    ///
    /// Driven by *observing* that a game is over, not by having played the
    /// last move. The player who delivers mate signs as a side effect of
    /// moving; their opponent never moves again, so without this they would
    /// never sign at all and no game would ever be rated.
    CertifyGame(GameId),
    /// Publish my nickname to my profile contract and the ranking.
    SetNickname(String),
    /// Heartbeat, optionally announcing the game being watched.
    Heartbeat {
        watching: Option<GameId>,
    },
    /// Ask the delegate for the stored account, storing ours if it has none.
    LoadAccount,
    /// Ask every retired delegate whether it still holds an account seed.
    ///
    /// Sent only when the current delegate has none, which is the one moment
    /// recovery is both possible and safe: adopting an old seed when the
    /// current delegate already has one would overwrite a live identity with an
    /// abandoned one.
    ProbeLegacyDelegates,
    /// Hand this session's account to the delegate for safekeeping.
    StoreAccount,
    ImportAccount {
        seed: [u8; 32],
    },
    LoadLegacyNickname(DelegateKey),
    LoadLegacyTheme(DelegateKey),
    /// Ask the delegate for the stored nickname.
    LoadNickname,
    LoadTheme,
    SaveTheme {
        player: [u8; 32],
        theme: crate::theme::Theme,
    },

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
    /// Announce that the app has moved to a new contract address.
    ///
    /// Unlike the notices above this one is *enforced* where it can be: the
    /// lobby stops accepting new listings. Games already under way are
    /// untouched, and live in their own contracts regardless. An empty
    /// `new_address` calls the migration off.
    AnnounceMigration {
        new_address: String,
        message: String,
    },
}

/// A handle the views use to talk to the sync loop.
pub type Sync = Coroutine<Cmd>;

/// Bridges the WebApi callback (which is not a future) into the loop.
enum Event {
    Response(Box<HostResponse>),
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
                    Ok(response) => resp_tx.unbounded_send(Event::Response(Box::new(response))),
                    Err(e) => resp_tx.unbounded_send(Event::Failed(freenet::error_message(e))),
                };
            },
            move |e| {
                let _ = err_tx.unbounded_send(Event::Failed(freenet::api_error(e)));
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
                            .map_err(|e| format!("could not register the delegate: {}", freenet::api_error(e)))
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
                        if let Some(followup) = handle_response(&mut state, &instances, *response) {
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
        .map_err(|e| format!("could not publish the lobby: {}", freenet::api_error(e)))?;

    // GET with subscribe=true, and nothing else: a separate SUBSCRIBE sent
    // straight after would arrive before this GET has primed the local store,
    // and the node rejects subscribing to a contract whose code it does not
    // hold yet ("contract WASM/parameters not cached locally"). The GET carries
    // the subscription, so the second message was both redundant and the source
    // of that error.
    api.send(freenet::get_request(id, true))
        .await
        .map_err(|e| format!("could not fetch the lobby: {}", freenet::api_error(e)))
}

async fn handle_cmd(
    api: &mut WebApi,
    state: &mut Signal<AppState>,
    instances: &mut HashMap<ContractInstanceId, Subject>,
    cmd: Cmd,
) -> Result<(), String> {
    let account = state.with(|s| s.account.clone());
    let key = &account.key;
    let now = now_ms();

    match cmd {
        Cmd::CreateGame {
            color,
            time_control,
            challenged,
        } => {
            if !state.with(AppState::can_create_game) {
                return Err(
                    "New games are unavailable. Check your connection and the lobby notice."
                        .to_string(),
                );
            }
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
            .map_err(|e| format!("could not publish the game: {}", freenet::api_error(e)))?;

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
                    .map_err(|e| format!("could not fetch the game: {}", freenet::api_error(e)));
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
            .map_err(|e| format!("could not offer to play: {}", freenet::api_error(e)))?;
            // GET as well as subscribe: subscribing only delivers *future*
            // changes, so without this the board stays empty until the
            // opponent happens to move.
            api.send(freenet::get_request(instance.id(), true))
                .await
                .map_err(|e| format!("could not fetch the game: {}", freenet::api_error(e)))?;

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

        Cmd::DeclineChallenge(game_id) => {
            let creator = lookup_creator(state, game_id)
                .ok_or_else(|| "that game is not in the lobby".to_string())?;
            let decline = chess_core::game::opponent::SignedDecline::new(key, &game_id, now);

            // Publish to the game first: that is the authoritative record, and
            // it is what the challenger's own client reads.
            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                opponent: Some(OpponentDelta::decline(decline.clone())),
                ..Default::default()
            };
            api.send(freenet::update_request(
                instance.key(),
                freenet::encode(&delta)?,
            ))
            .await
            .map_err(|e| format!("could not decline the challenge: {}", freenet::api_error(e)))?;

            // Mirror into the lobby so the listing stops being offered to
            // anyone, including us — otherwise the challenge would keep showing
            // until someone happened to open the game itself.
            if let Some(mut entry) = state.with(|s| s.lobby.games.get(&game_id).cloned()) {
                entry.declined = Some(decline);
                push_lobby_entry(api, entry).await?;
            }
            Ok(())
        }

        Cmd::ResendOffer(game_id) => {
            let (creator, game) = match state.with(|s| {
                s.creators
                    .get(&game_id)
                    .copied()
                    .zip(s.games.get(&game_id).cloned())
            }) {
                Some(pair) => pair,
                None => return Ok(()),
            };
            let me = key.verifying_key();
            // Only while we are still waiting, and only our own offer.
            if game.opponent.get().is_some() || !game.opponent.has_offer_from(&me) {
                return Ok(());
            }
            let Some(mine) = game.opponent.proposal_offer_by(&me).cloned() else {
                return Ok(());
            };
            let instance = freenet::game_instance(creator, game_id)?;
            let delta = chess_core::game::ChessGameStateV1Delta {
                opponent: Some(OpponentDelta::offer(mine)),
                ..Default::default()
            };
            let _ = api
                .send(freenet::update_request(
                    instance.key(),
                    freenet::encode(&delta)?,
                ))
                .await;
            Ok(())
        }

        Cmd::SeatChallenger(game_id) => {
            // Fired by a timer, so it lands whenever a game is in view —
            // including the seconds before that game has arrived from the
            // network. Nobody asked for anything, so there is nothing to
            // report: erroring here put "that game is not loaded" on screen
            // over a page that was simply still loading.
            let Some((creator, game)) = state.with(|s| {
                s.creators
                    .get(&game_id)
                    .copied()
                    .zip(s.games.get(&game_id).cloned())
            }) else {
                return Ok(());
            };

            // Only the creator can seat anyone: anyone else signing this
            // produces a value every peer rejects, so bailing out here saves
            // the round trip.
            if creator != key.verifying_key() {
                return Ok(());
            }

            let acceptance = match game.opponent.accepted() {
                // Already seated, and no move has been played: re-publish the
                // SAME acceptance.
                //
                // A single lost delta is ordinary in a P2P network, and this
                // handshake used to publish exactly once. When the acceptance
                // failed to reach the challenger's node the two disagreed
                // permanently — one saw a game under way, the other sat on
                // "waiting for the creator", and nothing ever retried. It
                // happened in a real game.
                //
                // Re-sending is idempotent (the merge keeps one acceptance per
                // creator by a total order), and it stops the moment a move is
                // played, so a started game costs nothing.
                Some(existing) if game.moves.is_empty() => existing.clone(),
                Some(_) => return Ok(()),
                None => {
                    // Take the first offer in the map's deterministic order.
                    // Any choice is sound — the point is that the choice is
                    // *ours* — and a stable one keeps two tabs of the same
                    // account from countersigning different challengers.
                    let Some(join) = game.opponent.pending_offers().first().map(|j| (*j).clone())
                    else {
                        return Ok(());
                    };
                    SignedAcceptance::new(key, &game_id, join)
                }
            };
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
            .map_err(|e| format!("could not accept the challenger: {}", freenet::api_error(e)))?;

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
                .map_err(|e| format!("could not fetch the game: {}", freenet::api_error(e)))
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

            validate_move(&game, &key.verifying_key(), mv)?;

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
            .map_err(|e| format!("could not send the move: {}", freenet::api_error(e)))?;

            // Apply locally so the board responds immediately; the network
            // update will confirm the same thing.
            let mut updated = game.clone();
            updated.moves.moves.insert(ply, authorized);
            let params = chess_core::game::ChessGameParametersV1 { creator, game_id };
            updated.prune(&params)?;

            let finished = updated.result().is_over();
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            publish_snapshot(api, key, game_id, &updated, now).await?;
            if finished {
                certify_and_file(api, state, key, game_id, &updated, now).await?;
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
            if game.result().is_over() {
                return Err("the game is already over".to_string());
            }
            if game.player_keys().is_none() || game.color_of(&key.verifying_key()).is_none() {
                return Err("you are not playing in this game".to_string());
            }
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
            .map_err(|e| format!("could not resign: {}", freenet::api_error(e)))?;

            let mut updated = game.clone();
            updated.conclusion = ConclusionV1::single(conclusion);
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            publish_snapshot(api, key, game_id, &updated, now).await?;
            certify_and_file(api, state, key, game_id, &updated, now).await
        }

        Cmd::AttestClock(game_id) => {
            // Same as above: a timer fires this the moment the route names a
            // game, which is before the game has arrived. Silence, not an
            // error — this is the one that actually reached the screen.
            let Some((creator, game)) = state.with(|s| {
                s.creators
                    .get(&game_id)
                    .copied()
                    .zip(s.games.get(&game_id).cloned())
            }) else {
                return Ok(());
            };

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
            .map_err(|e| {
                format!(
                    "could not publish the clock attestation: {}",
                    freenet::api_error(e)
                )
            })?;

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
            // Driven by a timer now rather than a button, so a game that has not
            // arrived yet is the normal early case, not a fault.
            let Some((creator, game)) = state.with(|s| {
                s.creators
                    .get(&game_id)
                    .copied()
                    .zip(s.games.get(&game_id).cloned())
            }) else {
                return Ok(());
            };
            if game.result().is_over() {
                return Ok(());
            }

            let Some(my_color) = game.color_of(&key.verifying_key()) else {
                return Ok(()); // spectating
            };
            let at_ply = game.moves.move_list().len() as u32;
            // The deadline is derived from the opponent's own attestations, so
            // the client can name it exactly rather than asserting a time.
            let Some(provable) = game.timeout_provable_at(my_color.opposite(), at_ply) else {
                return Ok(());
            };
            if now < provable {
                return Ok(());
            }

            // Wait for the clock to actually run out, not merely for the claim
            // to become admissible.
            //
            // The contract admits a claim once the opponent's attestations have
            // gone stale, which happens long before their clock empties — a
            // player who steps away with twenty minutes in hand could be claimed
            // against in under a minute. Over the board you do not lose for
            // being away; your clock runs and you lose when it reaches zero.
            // That is what this waits for.
            //
            // Nothing in the contract has to change for it. `at` is the only
            // thing validated, and it is still named exactly — a contract has no
            // clock, so it cannot tell that we sat on the claim, and the value
            // stays the honest instant the state first supported it.
            //
            // An abandoned game still resolves; it resolves when the abandoner
            // would have flagged anyway, which is the same answer a real board
            // gives.
            if game.time_remaining(my_color.opposite(), now) > 0 {
                return Ok(());
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
            .map_err(|e| format!("could not claim the timeout: {}", freenet::api_error(e)))?;

            let mut updated = game.clone();
            updated.conclusion = ConclusionV1::single(conclusion);
            publish_snapshot(api, key, game_id, &updated, provable).await?;
            state.with_mut(|s| {
                s.games.insert(game_id, updated.clone());
            });
            certify_and_file(api, state, key, game_id, &updated, provable).await
        }

        Cmd::CertifyGame(game_id) => {
            let game = match state.with(|s| s.games.get(&game_id).cloned()) {
                Some(g) => g,
                None => return Ok(()),
            };
            if !game.result().is_over() {
                return Ok(());
            }
            certify_and_file(api, state, key, game_id, &game, now).await
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
            let store = chess_delegate_protocol::ChessDelegateRequest::SetNickname {
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
            .map_err(|e| format!("could not publish the nickname: {}", freenet::api_error(e)))
        }

        Cmd::LoadNickname => {
            let payload = chess_delegate_protocol::ChessDelegateRequest::GetNickname
                .to_bytes()
                .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| {
                    format!(
                        "could not ask the delegate for the nickname: {}",
                        freenet::api_error(e)
                    )
                })
        }

        Cmd::LoadLegacyNickname(source) => {
            if state.read().pending_import.is_some()
                || !state.with(|s| {
                    s.legacy_account
                        .as_ref()
                        .is_some_and(|(key, player)| key == &source && *player == s.me())
                })
            {
                return Ok(());
            }
            let payload = chess_delegate_protocol::ChessDelegateRequest::GetNickname.to_bytes()?;
            api.send(freenet::delegate_request_to(source, payload))
                .await
                .map_err(|e| format!("could not recover the nickname: {}", freenet::api_error(e)))
        }

        Cmd::ImportAccount { seed } => {
            let payload =
                chess_delegate_protocol::ChessDelegateRequest::StoreAccount { seed }.to_bytes()?;
            let nonce: [u8; 16] = rand::random();
            let context = [b"freechess:import:v1:".as_slice(), nonce.as_slice()].concat();
            state.with_mut(|s| {
                s.pending_import = Some(seed);
                s.pending_import_context = Some(context.clone());
            });
            let result = api
                .send(freenet::delegate_request_with_context(payload, context))
                .await
                .map_err(|_| "could not import the account: node connection failed".to_string());
            if result.is_err() {
                state.with_mut(|s| {
                    s.pending_import = None;
                    s.pending_import_context = None;
                });
            }
            result
        }

        Cmd::LoadAccount => {
            let payload = chess_delegate_protocol::ChessDelegateRequest::GetAccount
                .to_bytes()
                .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| {
                    format!(
                        "could not ask the delegate for the account: {}",
                        freenet::api_error(e)
                    )
                })
        }

        Cmd::ProbeLegacyDelegates => {
            let payload = chess_delegate_protocol::ChessDelegateRequest::GetAccount
                .to_bytes()
                .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            for code_hash in chess_delegate_protocol::LEGACY_DELEGATE_CODE_HASHES {
                let key = freenet::legacy_delegate_key(code_hash);
                // A node that never ran this delegate simply does not answer.
                // That is not an error worth surfacing — it is the normal case
                // for anyone who joined after the delegate changed.
                let _ = api
                    .send(freenet::delegate_request_to(key, payload.clone()))
                    .await;
            }
            Ok(())
        }

        Cmd::StoreAccount => {
            if !state.with(account_store_ready) {
                return Ok(());
            }
            let payload = chess_delegate_protocol::ChessDelegateRequest::StoreAccount {
                seed: key.to_bytes(),
            }
            .to_bytes()
            .map_err(|e| format!("could not encode the delegate request: {e}"))?;
            api.send(freenet::delegate_request(payload))
                .await
                .map_err(|e| format!("could not store the account: {}", freenet::api_error(e)))?;
            if state.with(|s| {
                s.legacy_account
                    .as_ref()
                    .is_some_and(|(_, player)| *player == s.me())
            }) {
                if !state.read().nickname_unset {
                    let payload = chess_delegate_protocol::ChessDelegateRequest::SetNickname {
                        nickname: account.nickname,
                    }
                    .to_bytes()?;
                    api.send(freenet::delegate_request(payload))
                        .await
                        .map_err(|e| {
                            format!("could not migrate the nickname: {}", freenet::api_error(e))
                        })?;
                }
                state.with_mut(|s| s.legacy_account = None);
            }
            Ok(())
        }

        Cmd::LoadTheme => {
            if !state.read().account_settled || state.read().pending_import.is_some() {
                return Ok(());
            }
            let request = chess_delegate_protocol::ChessDelegateRequest::GetTheme {
                player: key.verifying_key().to_bytes(),
            };
            let result = api
                .send(freenet::delegate_request(request.to_bytes()?))
                .await
                .map_err(|e| {
                    format!(
                        "could not load appearance preferences: {}",
                        freenet::api_error(e)
                    )
                });
            if let Err(error) = &result {
                state.with_mut(|s| fail_theme_probe(s, error.clone()));
            }
            result
        }

        Cmd::LoadLegacyTheme(source) => {
            if !state.with(|s| {
                s.pending_import.is_none()
                    && s.account_settled
                    && s.theme_player.is_none()
                    && s.theme_probe.as_ref().is_some_and(|probe| {
                        probe.player == key.verifying_key().to_bytes()
                            && probe.sources.first() == Some(&source)
                    })
            }) {
                return Ok(());
            }
            let request = chess_delegate_protocol::ChessDelegateRequest::GetTheme {
                player: key.verifying_key().to_bytes(),
            };
            state.with_mut(|s| {
                if let Some(probe) = s.theme_probe.as_mut() {
                    probe.deadline_ms = Some(now + 8000);
                }
            });
            let result = api
                .send(freenet::delegate_request_to(source, request.to_bytes()?))
                .await
                .map_err(|e| {
                    format!(
                        "could not recover appearance preferences: {}",
                        freenet::api_error(e)
                    )
                });
            if let Err(error) = &result {
                state.with_mut(|s| fail_theme_probe(s, error.clone()));
            }
            result
        }

        Cmd::SaveTheme { player, theme } => {
            if !state.read().account_settled || player != key.verifying_key().to_bytes() {
                return Ok(());
            }
            state.with_mut(|s| {
                s.theme_error = None;
                s.pending_theme = Some(theme.clone());
            });
            let request = chess_delegate_protocol::ChessDelegateRequest::SetTheme { player, theme };
            let result = api
                .send(freenet::delegate_request(request.to_bytes()?))
                .await
                .map_err(|e| {
                    format!(
                        "could not save appearance preferences: {}",
                        freenet::api_error(e)
                    )
                });
            if let Err(error) = &result {
                state.with_mut(|s| s.theme_error = Some(error.clone()));
            }
            result
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

        Cmd::AnnounceMigration {
            new_address,
            message,
        } => {
            let trimmed = new_address.trim().to_string();
            let m = if trimmed.is_empty() {
                chess_core::admin::Migration::cancel(key, now)
            } else {
                // Reject a malformed address here rather than publishing a
                // notice every peer will silently drop: the contract requires a
                // real 32-byte contract id, which is what lets clients render
                // it as a link.
                if chess_core::identity::GameId::from_base58(&trimmed).is_none() {
                    return Err("that is not a valid contract address".to_string());
                }
                chess_core::admin::Migration::announce(key, trimmed, message, now)
            };
            push_admin(api, vec![chess_core::admin::AdminAction::Migrate(m)]).await
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
            .map_err(|e| format!("could not publish presence: {}", freenet::api_error(e)))
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
    .map_err(|e| {
        format!(
            "could not publish the administrative action: {}",
            freenet::api_error(e)
        )
    })
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
    .map_err(|e| format!("could not update the lobby: {}", freenet::api_error(e)))
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
        declined: game.opponent.declined.clone(),
        snapshot: Some(snapshot),
    };
    push_lobby_entry(api, entry).await
}

/// Once a game is decided, take this player's part in certifying it, and file
/// the result the moment both halves are present.
///
/// Two steps, and which one runs depends only on what is already in state:
///
/// 1. **Agree on the record.** A rating is trustworthy because both players
///    signed the same one, and they cannot derive it independently — the finish
///    time is a wall clock and the pre-game ratings come from the lobby. So the
///    draft is published, and both sides converge on whichever wins a total
///    order. See [`chess_core::game::certification`].
/// 2. **File it.** Once both signatures are over identical bytes, anyone can
///    assemble the certificate; this client files its own ranking entry from
///    it. Filing is per player and self-signed, so neither side depends on the
///    other doing their part.
///
/// A game whose opponent never signs simply stays unrated, and remains
/// replayable from its own contract regardless.
async fn certify_and_file(
    api: &mut WebApi,
    state: &mut Signal<AppState>,
    key: &ed25519_dalek::SigningKey,
    game_id: GameId,
    game: &ChessGameStateV1,
    now: i64,
) -> Result<(), String> {
    let (white, black) = match game.player_keys() {
        Some(pair) => pair,
        None => return Ok(()),
    };
    let me = key.verifying_key();
    if me != white && me != black {
        return Ok(()); // spectating: nothing to sign
    }
    let creator = match lookup_creator(state, game_id) {
        Some(c) => c,
        None => return Ok(()),
    };
    let instance = freenet::game_instance(creator, game_id)?;

    // Step 2 first: if the two halves already agree, there is nothing left to
    // negotiate and the only thing to do is file.
    if let Some(certificate) = game.certification.certificate(game) {
        return file_certificate(api, state, key, certificate).await;
    }

    // Step 1. Prefer the draft already on the table over inventing another:
    // two drafts never assemble, and every extra one is another round trip.
    let mine = game.certification.proposal_by(&me);
    let candidate = match game.certification.winning_draft() {
        Some(draft) if acceptable(state, draft) => Some(draft.clone()),
        // Their draft says something we disagree with — most likely a rating
        // neither of us has converged on yet. Refuse it and offer our own; if
        // they hold to theirs the game stays unrated, which is the right way
        // to fail. Signing a rating we believe to be wrong is not.
        Some(_) => CertificateDraft::from_game(
            game,
            game_id,
            now,
            observed_rating(state, white),
            observed_rating(state, black),
        ),
        None => CertificateDraft::from_game(
            game,
            game_id,
            now,
            observed_rating(state, white),
            observed_rating(state, black),
        ),
    };
    let Some(draft) = candidate else {
        return Ok(());
    };
    // Already signed exactly this: re-publishing would be a no-op on every
    // peer, so stop rather than chatter every time a game view refreshes.
    if mine.map(|p| p.draft == draft).unwrap_or(false) {
        return Ok(());
    }

    let proposal = chess_core::game::certification::SignedCertificateProposal::new(key, draft);
    let delta = chess_core::game::ChessGameStateV1Delta {
        certification: Some(vec![proposal]),
        ..Default::default()
    };
    api.send(freenet::update_request(
        instance.key(),
        freenet::encode(&delta)?,
    ))
    .await
    .map_err(|e| {
        format!(
            "could not publish the certificate signature: {}",
            freenet::api_error(e)
        )
    })?;

    state.with_mut(|s| {
        s.games.insert(game_id, game.clone());
    });
    Ok(())
}

/// The rating this client currently sees for `player`.
fn observed_rating(state: &Signal<AppState>, player: VerifyingKey) -> i32 {
    state.with(|s| s.lobby.leaderboard.rating_of(PlayerId::from(&player)))
}

/// Whether we are willing to put our name to someone else's draft.
///
/// Everything else about the draft is already checked by the contract against
/// the game's own state. The ratings are not, and cannot be: they live in the
/// lobby, and a contract cannot read another contract. They rest entirely on
/// this check — a player who signs a draft claiming their opponent was rated
/// 3000 has handed them a win worth far more than it should be.
fn acceptable(state: &Signal<AppState>, draft: &CertificateDraft) -> bool {
    draft.white_rating_before == observed_rating(state, draft.white)
        && draft.black_rating_before == observed_rating(state, draft.black)
}

/// File a finished certificate into the ranking.
///
/// The entry is signed by this player and covers only this player, so both
/// sides file independently and neither waits on the other. Re-filing is
/// harmless: the leaderboard keeps the entry with the higher game count and
/// settles ties by a total order, so a repeat merges to a no-op.
async fn file_certificate(
    api: &mut WebApi,
    state: &mut Signal<AppState>,
    key: &ed25519_dalek::SigningKey,
    certificate: chess_core::certificate::GameCertificate,
) -> Result<(), String> {
    // Never file something that does not stand up on its own — a malformed
    // entry is rejected by every peer, and finding out here is cheaper.
    certificate.verify()?;

    let me = PlayerId::from(&key.verifying_key());
    let Some(score) = certificate.score_for(me) else {
        return Ok(());
    };
    let (rating_before, opponent_rating) = match certificate.color_of(me) {
        Some(chess_core::chess::Color::White) => (
            certificate.white_rating_before,
            certificate.black_rating_before,
        ),
        Some(chess_core::chess::Color::Black) => (
            certificate.black_rating_before,
            certificate.white_rating_before,
        ),
        None => return Ok(()),
    };

    // Only file a game *newer* than the one our entry already cites.
    //
    // The guard used to be "is this the game our entry names?", which is only
    // correct while there is one finished game. With two, each takes it in
    // turns to find the other named and re-file itself — and every re-filing
    // increments `games_played`, which changes the K factor, which changes the
    // rating. The leaderboard keeps whichever entry has the higher count, so
    // the two ratings visibly oscillated forever.
    //
    // Comparing finish times is durable across reloads (it needs no memory of
    // what this device has done) and settles: each game is filed exactly once,
    // and `games_played` advances by exactly one per game.
    let (nickname, games_played, already_counted) = state.with(|s| {
        let existing = s.lobby.leaderboard.get(me);
        (
            s.account.nickname.clone(),
            existing.map(|e| e.games_played).unwrap_or(0) + 1,
            existing
                .map(|e| certificate.finished_at <= e.last_game.finished_at)
                .unwrap_or(false),
        )
    });
    if already_counted {
        return Ok(());
    }

    let rating = chess_core::elo::apply_result(
        rating_before,
        games_played.saturating_sub(1),
        opponent_rating,
        score,
    );
    let entry = chess_core::leaderboard::RankEntry::new(
        key,
        nickname,
        rating,
        games_played,
        certificate,
        now_ms(),
    );
    let delta = chess_core::lobby::LobbyStateV1Delta {
        leaderboard: Some(vec![entry]),
        ..Default::default()
    };
    let instance = freenet::lobby_instance()?;
    api.send(freenet::update_request(
        instance.key(),
        freenet::encode(&delta)?,
    ))
    .await
    .map_err(|e| {
        format!(
            "could not file the ranking entry: {}",
            freenet::api_error(e)
        )
    })?;

    state.with_mut(|s| s.message = Some(format!("game rated: {rating}")));
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
    if let HostResponse::DelegateResponse { key, values } = &response {
        return handle_delegate_response(state, key, values);
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
    origin: &DelegateKey,
    values: &[freenet_stdlib::prelude::OutboundDelegateMsg],
) -> Option<Cmd> {
    use chess_delegate_protocol::ChessDelegateResponse;
    use freenet_stdlib::prelude::OutboundDelegateMsg;
    let mut followup = None;
    for value in values {
        let OutboundDelegateMsg::ApplicationMessage(msg) = value else {
            continue;
        };
        let Ok(response) = ChessDelegateResponse::from_bytes(&msg.payload) else {
            continue;
        };
        let action =
            state.with_mut(|s| apply_delegate_reply(s, origin, msg.context.as_ref(), response));
        if followup.is_none() {
            followup = action;
        }
    }
    followup
}

fn account_store_ready(state: &AppState) -> bool {
    state.pending_import.is_none()
        && !state.legacy_nickname_pending
        && state.legacy_theme_source.is_none()
}

#[derive(Clone)]
struct ThemeProbe {
    player: [u8; 32],
    sources: Vec<DelegateKey>,
    deadline_ms: Option<i64>,
    failed: bool,
}

fn fail_theme_probe(state: &mut AppState, message: String) {
    if let Some(probe) = state.theme_probe.as_mut() {
        probe.failed = true;
    }
    state.theme_error = Some(message);
}

fn start_theme_probe(state: &mut AppState, player: [u8; 32]) -> Option<Cmd> {
    let sources: Vec<_> = chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES
        .iter()
        .map(freenet::legacy_delegate_key)
        .collect();
    let Some(source) = sources.first().cloned() else {
        crate::theme::receive(state, player, None);
        return None;
    };
    state.theme_probe = Some(ThemeProbe {
        player,
        sources,
        deadline_ms: None,
        failed: false,
    });
    Some(Cmd::LoadLegacyTheme(source))
}

fn next_theme_probe(state: &mut AppState) -> Option<Cmd> {
    let mut probe = state.theme_probe.take()?;
    probe.sources.remove(0);
    if let Some(source) = probe.sources.first().cloned() {
        probe.deadline_ms = None;
        probe.failed = false;
        state.theme_probe = Some(probe);
        Some(Cmd::LoadLegacyTheme(source))
    } else {
        state.legacy_theme_source = None;
        crate::theme::receive(state, probe.player, None);
        state.legacy_account.as_ref().map(|_| Cmd::StoreAccount)
    }
}

pub fn check_theme_recovery(state: &mut AppState, now: i64) -> Option<Cmd> {
    let probe = state.theme_probe.as_ref()?;
    if state.pending_import.is_none()
        && state.account_settled
        && state.theme_player.is_none()
        && probe.player == state.account.key.verifying_key().to_bytes()
        && !probe.failed
        && probe.deadline_ms.is_some_and(|deadline| now >= deadline)
    {
        next_theme_probe(state)
    } else {
        None
    }
}

#[cfg(test)]
fn apply_delegate_response(
    state: &mut AppState,
    origin: &DelegateKey,
    response: chess_delegate_protocol::ChessDelegateResponse,
) -> Option<Cmd> {
    apply_delegate_reply(state, origin, &[], response)
}

fn apply_delegate_reply(
    state: &mut AppState,
    origin: &DelegateKey,
    context: &[u8],
    response: chess_delegate_protocol::ChessDelegateResponse,
) -> Option<Cmd> {
    use chess_delegate_protocol::ChessDelegateResponse;
    let current = origin == &freenet::delegate_key();
    let legacy = chess_delegate_protocol::LEGACY_DELEGATE_CODE_HASHES
        .iter()
        .any(|hash| origin == &freenet::legacy_delegate_key(hash));
    if !current && !legacy {
        return None;
    }
    if current {
        clear_registration_error(state);
    }
    if state.pending_import.is_some()
        && matches!(
            &response,
            ChessDelegateResponse::Account { .. }
                | ChessDelegateResponse::Nickname { .. }
                | ChessDelegateResponse::Theme { .. }
        )
    {
        return None;
    }
    match response {
        ChessDelegateResponse::Account { seed: Some(seed) } => {
            if state.account_settled || (!current && !state.legacy_probed) {
                return None;
            }
            let key = ed25519_dalek::SigningKey::from_bytes(&seed);
            if state.account.key.to_bytes() != seed {
                state.account.key = key;
                state.games.clear();
                state.theme_player = None;
                state.stored_theme = None;
                state.pending_theme = None;
                state.theme_error = None;
                state.theme_probe = None;
                if !current {
                    state.account.nickname = Account::default_nickname(state.me());
                    state.nickname_unset = true;
                }
            }
            state.account_settled = true;
            #[cfg(target_arch = "wasm32")]
            crate::identity::save_local(&state.account.key);
            if current {
                state.legacy_account = None;
                state.legacy_nickname_pending = false;
                state.legacy_theme_source = None;
                Some(Cmd::StoreAccount)
            } else {
                state.legacy_account = Some((origin.clone(), state.me()));
                state.legacy_nickname_pending = true;
                state.legacy_theme_source =
                    chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES
                        .iter()
                        .any(|hash| origin == &freenet::legacy_delegate_key(hash))
                        .then(|| origin.clone());
                Some(Cmd::LoadLegacyNickname(origin.clone()))
            }
        }
        ChessDelegateResponse::Account { seed: None } if current => {
            if state.account_settled || state.legacy_probed {
                return None;
            }
            if !chess_delegate_protocol::LEGACY_DELEGATE_CODE_HASHES.is_empty() {
                state.legacy_probed = true;
                Some(Cmd::ProbeLegacyDelegates)
            } else {
                state.account_settled = true;
                Some(Cmd::StoreAccount)
            }
        }
        ChessDelegateResponse::Nickname { nickname } => {
            let recovering = state
                .legacy_account
                .as_ref()
                .is_some_and(|(source, player)| source == origin && *player == state.me());
            if !current && !recovering {
                return None;
            }
            if current && state.legacy_account.is_some() {
                return None;
            }
            if recovering {
                state.legacy_nickname_pending = false;
            }
            if let Some(name) = nickname.filter(|name| !name.trim().is_empty()) {
                if current || state.nickname_unset {
                    state.account.nickname = name;
                    state.nickname_unset = false;
                    #[cfg(target_arch = "wasm32")]
                    crate::identity::save_local_nickname(&state.account.nickname);
                }
            }
            recovering.then_some(Cmd::StoreAccount)
        }
        ChessDelegateResponse::AccountStored { seed } if current => {
            if state.pending_import == Some(seed) {
                state.pending_import = None;
                state.pending_import_context = None;
                state.legacy_account = None;
                state.legacy_nickname_pending = false;
                state.legacy_theme_source = None;
                state.account.key = ed25519_dalek::SigningKey::from_bytes(&seed);
                state.account_settled = true;
                state.games.clear();
                state.theme_player = None;
                state.stored_theme = None;
                state.pending_theme = None;
                state.theme_error = None;
                state.theme_probe = None;
                #[cfg(target_arch = "wasm32")]
                {
                    crate::identity::save_local(&state.account.key);
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().reload();
                    }
                }
            }
            None
        }
        ChessDelegateResponse::Theme { player, theme } => {
            if !state.account_settled
                || player != state.account.key.verifying_key().to_bytes()
                || state.theme_player.is_some()
            {
                return None;
            }
            let recovering = state.theme_probe.as_ref().is_some_and(|probe| {
                probe.player == player && probe.sources.first() == Some(origin)
            });
            if !current && !recovering {
                return None;
            }
            if current && theme.is_none() {
                if let Some(probe) = state.theme_probe.as_ref() {
                    return probe
                        .failed
                        .then(|| Cmd::LoadLegacyTheme(probe.sources[0].clone()));
                }
                return start_theme_probe(state, player);
            }
            if theme.as_ref().is_some_and(|theme| !theme.valid()) {
                fail_theme_probe(state, "stored appearance preferences are invalid".into());
                return None;
            }
            if recovering && theme.is_none() {
                return next_theme_probe(state);
            }
            if recovering {
                state.legacy_theme_source = Some(origin.clone());
            }
            state.theme_probe = None;
            let migrate = state.legacy_theme_source.is_some();
            let copied = theme.clone();
            crate::theme::receive(state, player, theme);
            if migrate {
                copied.map(|theme| Cmd::SaveTheme { player, theme })
            } else {
                None
            }
        }
        ChessDelegateResponse::ThemeSaved { player, theme } if current => {
            let matching = state.account_settled
                && player == state.account.key.verifying_key().to_bytes()
                && state.pending_theme.as_ref() == Some(&theme)
                && theme.valid();
            crate::theme::saved(state, player, theme);
            if matching
                && state.legacy_theme_source.take().is_some()
                && state.legacy_account.is_some()
            {
                Some(Cmd::StoreAccount)
            } else {
                None
            }
        }
        ChessDelegateResponse::Error { message }
            if state.legacy_theme_source.as_ref() == Some(origin)
                || state
                    .theme_probe
                    .as_ref()
                    .is_some_and(|probe| probe.sources.first() == Some(origin)) =>
        {
            let message = freenet::error_message(message);
            fail_theme_probe(state, message.clone());
            state.message = Some(format!("delegate: {message}"));
            None
        }
        ChessDelegateResponse::Error { message } if current => {
            let message = freenet::error_message(message);
            if !context.is_empty()
                && state
                    .pending_import_context
                    .as_deref()
                    .is_some_and(|expected| expected == context)
            {
                state.pending_import = None;
                state.pending_import_context = None;
            } else {
                fail_theme_probe(state, message.clone());
            }
            state.message = Some(format!("delegate: {message}"));
            None
        }
        _ => None,
    }
}

fn clear_registration_error(state: &mut AppState) {
    if state
        .message
        .as_deref()
        .is_some_and(|message| message.contains("missing delegate"))
    {
        state.message = None;
    }
}

fn validate_move(game: &ChessGameStateV1, player: &VerifyingKey, mv: Move) -> Result<(), String> {
    if game.result().is_over() {
        return Err("the game is already over".to_string());
    }
    if game.player_keys().is_none() {
        return Err("waiting for an opponent to join".to_string());
    }
    let replay = game.replay();
    if game.color_of(player) != Some(replay.side_to_move()) {
        return Err("it is not your turn".to_string());
    }
    if !replay.is_legal(mv) {
        return Err("illegal move".to_string());
    }
    Ok(())
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
    fn unrelated_delegate_error_import_regression() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let current = freenet::delegate_key();
        state.pending_import = Some([7; 32]);
        apply_delegate_response(
            &mut state,
            &current,
            Reply::Error {
                message: "could not decode stored theme".into(),
            },
        );
        assert_eq!(state.pending_import, Some([7; 32]));
        apply_delegate_response(&mut state, &current, Reply::AccountStored { seed: [7; 32] });
        assert_eq!(state.account.key.to_bytes(), [7; 32]);
    }

    #[test]
    fn inactive_account_theme_recovery_regression() {
        use chess_delegate_protocol::{ChessDelegateResponse as Reply, Theme};
        let mut state = recovery_state();
        state.account_settled = true;
        let player = state.account.key.verifying_key().to_bytes();
        let current = freenet::delegate_key();
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &current,
                Reply::Theme {
                    player,
                    theme: None
                }
            ),
            Some(Cmd::LoadLegacyTheme(_))
        ));
        assert!(state.theme_player.is_none());
        let source = freenet::legacy_delegate_key(
            &chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES[0],
        );
        let theme = Theme {
            dark_square: "#123456".into(),
            ..Theme::default()
        };
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &source,
                Reply::Theme {
                    player,
                    theme: Some(theme.clone())
                }
            ),
            Some(Cmd::SaveTheme { .. })
        ));
        assert_eq!(state.stored_theme, Some(theme.clone()));
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
        assert!(state.legacy_account.is_none());
        state.pending_theme = Some(theme.clone());
        apply_delegate_response(&mut state, &current, Reply::ThemeSaved { player, theme });
        assert!(state.legacy_theme_source.is_none());
    }

    #[test]
    fn missing_or_silent_retired_themes_finish_but_errors_require_recovery() {
        use chess_delegate_protocol::{ChessDelegateResponse as Reply, Theme};
        let source = freenet::legacy_delegate_key(
            &chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES[0],
        );
        for silent in [false, true] {
            let mut state = recovery_state();
            state.account_settled = true;
            let player = state.account.key.verifying_key().to_bytes();
            start_theme_probe(&mut state, player);
            if silent {
                state.theme_probe.as_mut().unwrap().deadline_ms = Some(100);
                check_theme_recovery(&mut state, 99);
                assert!(state.theme_player.is_none());
                check_theme_recovery(&mut state, 100);
            } else {
                apply_delegate_response(
                    &mut state,
                    &source,
                    Reply::Theme {
                        player,
                        theme: None,
                    },
                );
            }
            assert_eq!(state.theme_player, Some(player));
            assert!(state.stored_theme.is_none());
            assert!(state.theme_probe.is_none());
        }
        let mut state = recovery_state();
        state.account_settled = true;
        let player = state.account.key.verifying_key().to_bytes();
        start_theme_probe(&mut state, player);
        state.theme_probe.as_mut().unwrap().deadline_ms = Some(100);
        apply_delegate_response(
            &mut state,
            &source,
            Reply::Error {
                message: "old theme is unreadable".into(),
            },
        );
        check_theme_recovery(&mut state, 1000);
        assert!(state.theme_player.is_none());
        assert_eq!(
            state.theme_error.as_deref(),
            Some("old theme is unreadable")
        );
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &freenet::delegate_key(),
                Reply::Theme {
                    player,
                    theme: None
                }
            ),
            Some(Cmd::LoadLegacyTheme(_))
        ));
        apply_delegate_response(
            &mut state,
            &source,
            Reply::Theme {
                player,
                theme: Some(Theme::default()),
            },
        );
        assert_eq!(state.theme_player, Some(player));
        assert!(state.theme_error.is_none());
    }

    #[test]
    fn only_a_matching_import_error_context_cancels_pending_import() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        state.pending_import = Some([7; 32]);
        state.pending_import_context = Some(b"import:1".to_vec());
        let current = freenet::delegate_key();
        for (origin, context) in [
            (current.clone(), b"theme:1".as_slice()),
            (current.clone(), b"import:old".as_slice()),
            (current.clone(), b"".as_slice()),
            (retired_delegate(), b"import:1".as_slice()),
        ] {
            apply_delegate_reply(
                &mut state,
                &origin,
                context,
                Reply::Error {
                    message: "unrelated".into(),
                },
            );
            assert_eq!(state.pending_import, Some([7; 32]));
        }
        apply_delegate_reply(
            &mut state,
            &current,
            b"import:1",
            Reply::Error {
                message: "account store rejected".into(),
            },
        );
        assert!(state.pending_import.is_none());
        assert!(state.pending_import_context.is_none());
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
    }

    #[test]
    fn timed_out_legacy_theme_remains_unwritten_and_can_recover_next_session() {
        use chess_delegate_protocol::{ChessDelegateResponse as Reply, Theme};
        let mut state = recovery_state();
        state.account_settled = true;
        let player = state.account.key.verifying_key().to_bytes();
        let source = freenet::legacy_delegate_key(
            &chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES[0],
        );
        state.legacy_account = Some((source.clone(), state.me()));
        state.legacy_theme_source = Some(source.clone());
        start_theme_probe(&mut state, player);
        state.theme_probe.as_mut().unwrap().deadline_ms = Some(100);
        assert!(matches!(
            check_theme_recovery(&mut state, 100),
            Some(Cmd::StoreAccount)
        ));
        assert!(state.legacy_theme_source.is_none());
        assert_eq!(state.theme_player, Some(player));
        assert!(state.stored_theme.is_none());
        assert!(state.pending_theme.is_none());

        let mut reloaded = recovery_state();
        reloaded.account_settled = true;
        assert!(matches!(
            apply_delegate_response(
                &mut reloaded,
                &freenet::delegate_key(),
                Reply::Theme {
                    player,
                    theme: None
                }
            ),
            Some(Cmd::LoadLegacyTheme(_))
        ));
        let previous = Theme {
            dark_square: "#123456".into(),
            ..Theme::default()
        };
        assert!(matches!(
            apply_delegate_response(
                &mut reloaded,
                &source,
                Reply::Theme {
                    player,
                    theme: Some(previous.clone())
                }
            ),
            Some(Cmd::SaveTheme { .. })
        ));
        assert_eq!(reloaded.stored_theme, Some(previous));
    }

    fn recovery_state() -> AppState {
        AppState::new(Account {
            key: ed25519_dalek::SigningKey::from_bytes(&[6; 32]),
            nickname: "Player".into(),
        })
    }

    fn retired_delegate() -> DelegateKey {
        chess_delegate_protocol::LEGACY_DELEGATE_CODE_HASHES
            .iter()
            .map(freenet::legacy_delegate_key)
            .find(|key| key != &freenet::delegate_key())
            .unwrap()
    }

    #[test]
    fn recovery_accepts_one_legacy_identity_and_preserves_its_nickname() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let current = freenet::delegate_key();
        let old = retired_delegate();
        assert!(matches!(
            apply_delegate_response(&mut state, &current, Reply::Account { seed: None }),
            Some(Cmd::ProbeLegacyDelegates)
        ));
        assert!(apply_delegate_response(&mut state, &old, Reply::Account { seed: None }).is_none());
        assert!(!state.account_settled);
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &old,
                Reply::Account {
                    seed: Some([7; 32])
                }
            ),
            Some(Cmd::LoadLegacyNickname(_))
        ));
        assert_eq!(state.account.key.to_bytes(), [7; 32]);
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &old,
                Reply::Nickname {
                    nickname: Some("Recovered".into())
                }
            ),
            Some(Cmd::StoreAccount)
        ));
        assert_eq!(state.account.nickname, "Recovered");
        assert!(!state.nickname_unset);
        apply_delegate_response(
            &mut state,
            &old,
            Reply::Account {
                seed: Some([8; 32]),
            },
        );
        apply_delegate_response(&mut state, &current, Reply::Account { seed: None });
        assert_eq!(state.account.key.to_bytes(), [7; 32]);
        state.account.key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        apply_delegate_response(
            &mut state,
            &old,
            Reply::Nickname {
                nickname: Some("Stale".into()),
            },
        );
        assert_eq!(state.account.nickname, "Recovered");
    }

    #[test]
    fn settled_current_and_unknown_sources_cannot_replace_identity() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let old = retired_delegate();
        state.account_settled = true;
        state.legacy_probed = true;
        apply_delegate_response(
            &mut state,
            &old,
            Reply::Account {
                seed: Some([7; 32]),
            },
        );
        apply_delegate_response(
            &mut state,
            &freenet::delegate_key(),
            Reply::Account {
                seed: Some([7; 32]),
            },
        );
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
        state.account_settled = false;
        let unknown = freenet::legacy_delegate_key(&[0xff; 32]);
        apply_delegate_response(
            &mut state,
            &unknown,
            Reply::Account {
                seed: Some([7; 32]),
            },
        );
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
        assert!(!state.account_settled);
    }

    #[test]
    fn import_requires_matching_current_delegate_ack_and_keeps_errors_visible() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let current = freenet::delegate_key();
        state.pending_import = Some([7; 32]);
        apply_delegate_response(&mut state, &current, Reply::Stored);
        apply_delegate_response(
            &mut state,
            &retired_delegate(),
            Reply::AccountStored { seed: [7; 32] },
        );
        apply_delegate_response(&mut state, &current, Reply::AccountStored { seed: [8; 32] });
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
        assert_eq!(state.pending_import, Some([7; 32]));
        apply_delegate_response(&mut state, &current, Reply::AccountStored { seed: [7; 32] });
        assert_eq!(state.account.key.to_bytes(), [7; 32]);
        assert_eq!(state.pending_import, None);
        state.pending_import = Some([8; 32]);
        state.pending_import_context = Some(b"import:2".to_vec());
        apply_delegate_response(
            &mut state,
            &current,
            Reply::Error {
                message: "store rejected".into(),
            },
        );
        assert_eq!(state.pending_import, Some([8; 32]));
        apply_delegate_reply(
            &mut state,
            &current,
            b"import:2",
            Reply::Error {
                message: "store rejected".into(),
            },
        );
        assert!(state.pending_import.is_none());
        assert_eq!(state.account.key.to_bytes(), [7; 32]);
        assert_eq!(state.message.as_deref(), Some("delegate: store rejected"));
    }

    #[test]
    fn pending_import_blocks_recovery_replies_and_account_writes() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let old = retired_delegate();
        state.account_settled = true;
        state.legacy_account = Some((old.clone(), state.me()));
        state.pending_import = Some([7; 32]);
        assert!(!account_store_ready(&state));
        assert!(apply_delegate_response(
            &mut state,
            &old,
            Reply::Nickname {
                nickname: Some("Old".into())
            }
        )
        .is_none());
        assert!(apply_delegate_response(
            &mut state,
            &freenet::delegate_key(),
            Reply::Account {
                seed: Some([8; 32])
            }
        )
        .is_none());
        assert_eq!(state.account.key.to_bytes(), [6; 32]);
        assert_eq!(state.account.nickname, "Player");
    }

    #[test]
    fn legacy_theme_migration_waits_for_matching_player_source_and_save_ack() {
        use chess_delegate_protocol::{ChessDelegateResponse as Reply, Theme};
        let mut state = recovery_state();
        let source = freenet::legacy_delegate_key(
            &chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES[0],
        );
        let current = freenet::delegate_key();
        let player = state.account.key.verifying_key().to_bytes();
        state.account_settled = true;
        state.legacy_account = Some((source.clone(), state.me()));
        state.legacy_nickname_pending = true;
        state.legacy_theme_source = Some(source.clone());
        let custom = Theme {
            dark_square: "#112233".into(),
            ..Theme::default()
        };
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &current,
                Reply::Theme {
                    player,
                    theme: None
                }
            ),
            Some(Cmd::LoadLegacyTheme(_))
        ));
        assert!(state.theme_player.is_none());
        apply_delegate_response(
            &mut state,
            &source,
            Reply::Theme {
                player: [0; 32],
                theme: Some(custom.clone()),
            },
        );
        apply_delegate_response(
            &mut state,
            &freenet::legacy_delegate_key(&chess_delegate_protocol::LEGACY_DELEGATE_CODE_HASHES[1]),
            Reply::Theme {
                player,
                theme: Some(custom.clone()),
            },
        );
        assert!(state.theme_player.is_none());
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &source,
                Reply::Theme {
                    player,
                    theme: Some(custom.clone())
                }
            ),
            Some(Cmd::SaveTheme { .. })
        ));
        assert!(!account_store_ready(&state));
        state.pending_theme = Some(custom.clone());
        apply_delegate_response(
            &mut state,
            &current,
            Reply::ThemeSaved {
                player,
                theme: Theme::default(),
            },
        );
        assert!(state.legacy_theme_source.is_some());
        assert!(matches!(
            apply_delegate_response(
                &mut state,
                &current,
                Reply::ThemeSaved {
                    player,
                    theme: custom.clone()
                }
            ),
            Some(Cmd::StoreAccount)
        ));
        assert!(!account_store_ready(&state));
        apply_delegate_response(
            &mut state,
            &source,
            Reply::Nickname {
                nickname: Some("Recovered".into()),
            },
        );
        assert!(account_store_ready(&state));
        assert_eq!(state.stored_theme, Some(custom));
        assert_eq!(state.account.nickname, "Recovered");
    }

    #[test]
    fn legacy_theme_failure_keeps_recovery_pending_instead_of_saving_defaults() {
        use chess_delegate_protocol::ChessDelegateResponse as Reply;
        let mut state = recovery_state();
        let source = freenet::legacy_delegate_key(
            &chess_delegate_protocol::LEGACY_THEME_DELEGATE_CODE_HASHES[0],
        );
        state.account_settled = true;
        state.legacy_theme_source = Some(source.clone());
        apply_delegate_response(
            &mut state,
            &source,
            Reply::Error {
                message: "could not read old preferences".into(),
            },
        );
        assert!(state.theme_player.is_none());
        assert!(state.legacy_theme_source.is_some());
        assert!(!account_store_ready(&state));
        assert_eq!(
            state.theme_error.as_deref(),
            Some("could not read old preferences")
        );
    }

    #[test]
    fn account_reply_does_not_drop_later_nickname_in_the_same_batch() {
        fn repro() -> Element {
            use chess_delegate_protocol::ChessDelegateResponse as Reply;
            use freenet_stdlib::prelude::{ApplicationMessage, OutboundDelegateMsg};
            let mut state = use_signal(recovery_state);
            let values = [
                Reply::Account {
                    seed: Some([7; 32]),
                },
                Reply::Nickname {
                    nickname: Some("Recovered".into()),
                },
            ]
            .into_iter()
            .map(|reply| {
                OutboundDelegateMsg::ApplicationMessage(
                    ApplicationMessage::new(reply.to_bytes().unwrap()).processed(true),
                )
            })
            .collect::<Vec<_>>();
            assert!(matches!(
                handle_delegate_response(&mut state, &freenet::delegate_key(), &values),
                Some(Cmd::StoreAccount)
            ));
            assert_eq!(state.read().account.nickname, "Recovered");
            rsx! {}
        }
        VirtualDom::new(repro).rebuild_in_place();
    }

    #[test]
    fn repeated_empty_account_reply_waits_for_legacy_recovery() {
        fn repro() -> Element {
            let mut state = use_signal(|| {
                let mut state = AppState::new(Account {
                    key: ed25519_dalek::SigningKey::from_bytes(&[6; 32]),
                    nickname: "Player".into(),
                });
                state.legacy_probed = true;
                state
            });
            let reply = chess_delegate_protocol::ChessDelegateResponse::Account { seed: None };
            let values = vec![
                freenet_stdlib::prelude::OutboundDelegateMsg::ApplicationMessage(
                    freenet_stdlib::prelude::ApplicationMessage::new(reply.to_bytes().unwrap())
                        .processed(true),
                ),
            ];
            assert!(
                handle_delegate_response(&mut state, &freenet::delegate_key(), &values).is_none()
            );
            assert!(!state.read().account_settled);
            rsx! {}
        }
        VirtualDom::new(repro).rebuild_in_place();
    }

    #[test]
    fn delegate_recovery_clears_registration_errors_without_hiding_other_messages() {
        let mut state = AppState::new(Account {
            key: ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
            nickname: "Player".into(),
        });
        state.message = Some("client error: missing delegate".into());
        clear_registration_error(&mut state);
        assert!(state.message.is_none());
        state.message = Some("could not publish move".into());
        clear_registration_error(&mut state);
        assert_eq!(state.message.as_deref(), Some("could not publish move"));
    }

    #[test]
    fn creating_games_requires_connection_settled_identity_and_an_open_lobby() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let mut state = AppState::new(Account {
            key: key.clone(),
            nickname: "Player".into(),
        });
        assert!(!state.can_create_game());
        state.status = ConnStatus::Online;
        assert!(!state.can_create_game());
        state.account_settled = true;
        assert!(state.can_create_game());
        state.lobby.administration.migration = Some(chess_core::admin::Migration::announce(
            &key,
            bs58::encode([2; 32]).into_string(),
            "Moved".into(),
            1,
        ));
        assert!(!state.can_create_game());
        state.lobby.administration.migration = Some(chess_core::admin::Migration::cancel(&key, 2));
        assert!(state.can_create_game());
    }

    #[test]
    fn move_commands_reject_waiting_spectators_wrong_turn_illegal_and_finished_games() {
        use chess_core::game::opponent::OpponentSlotV1;
        use ed25519_dalek::SigningKey;

        let white = SigningKey::from_bytes(&[1; 32]);
        let black = SigningKey::from_bytes(&[2; 32]);
        let spectator = SigningKey::from_bytes(&[3; 32]);
        let created_at = 1_700_000_000_000;
        let setup = GameSetup {
            creator_plays: Color::White,
            time_control: TimeControl::default(),
            created_at,
            pow_nonce: 0,
            creator_nickname: "White".to_string(),
            challenged: None,
        };
        let game_id = setup.derive_game_id(&white.verifying_key());
        let mut game = ChessGameStateV1 {
            setup: GameSetupV1(Some(setup.sign(&white, &game_id))),
            ..Default::default()
        };
        let e4 = Move::from_uci("e2e4").unwrap();
        assert!(validate_move(&game, &white.verifying_key(), e4).is_err());
        let join = SignedJoin::new(&black, &game_id, created_at + 1000, "Black".to_string());
        game.opponent = OpponentSlotV1 {
            seated: Some(SignedAcceptance::new(&white, &game_id, join)),
            ..Default::default()
        };
        assert!(validate_move(&game, &white.verifying_key(), e4).is_ok());
        assert!(validate_move(&game, &black.verifying_key(), e4).is_err());
        assert!(validate_move(&game, &spectator.verifying_key(), e4).is_err());
        assert!(validate_move(
            &game,
            &white.verifying_key(),
            Move::from_uci("e2e5").unwrap()
        )
        .is_err());
        game.conclusion = ConclusionV1::single(SignedConclusion::resign(
            &black,
            &game_id,
            0,
            created_at + 2000,
        ));
        assert!(game.result().is_over());
        assert!(validate_move(&game, &white.verifying_key(), e4).is_err());
    }

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
