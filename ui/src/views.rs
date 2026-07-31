//! The screens.

use crate::app::{format_clock, result_headline, AppState, Cmd, Route, Sync};
use crate::board::{captured_material, GameBoard, MiniBoard};
use crate::identity::now_ms;
use chess_core::account;
use chess_core::chess::{Board, Color, Move, PieceKind};
use chess_core::game::setup::TimeControl;
use chess_core::identity::{GameId, PlayerId};
use chess_core::lobby::LobbyEntry;
use chess_core::player::validate_nickname;
use dioxus::prelude::*;

/// Header: brand, connection state, notifications, account.
#[component]
pub fn TopBar(state: Signal<AppState>, sync: Sync) -> Element {
    let mut show_account = use_signal(|| false);
    let mut show_admin = use_signal(|| false);
    let mut show_notifications = use_signal(|| false);
    let s = state.read();
    let status = s.status;
    let nickname = s.account.nickname.clone();
    let rating = s.my_rating();
    let online = s.lobby.presence.online_count(now_ms());
    let is_admin = s.is_admin();
    // Direct challenges waiting on an answer. The count is of things still to
    // act on, not of things not yet read: a challenge someone accepted or
    // withdrew stops counting on its own, which needs no record of what this
    // device has seen and cannot drift out of step with the network.
    let challenges: Vec<_> = s
        .lobby
        .games
        .challenges_for(s.me())
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    // Visible to admins, and to everyone while nobody has claimed it — hiding
    // an unclaimed race would only mean whoever reads the source wins it.
    let show_admin_entry = is_admin || s.admin_unclaimed();
    drop(s);

    rsx! {
        div { class: "topbar",
            div {
                class: "brand",
                onclick: move |_| state.with_mut(|s| s.route = Route::Home),
                span { class: "mark", "\u{265E}" }
                span { "FreeChess" }
            }
            div { class: "spacer" }
            div { class: "conn",
                span { class: "dot {status.css()}" }
                span { "{status.label()}" }
            }
            div { class: "conn", "{online} online" }
            // A plain hyperlink, not a fetched resource — nothing is loaded
            // from outside, so the gateway's same-origin CSP is unaffected.
            a {
                class: "conn gh",
                href: "https://github.com/mguinhos/freechess",
                target: "_blank",
                rel: "noopener noreferrer",
                title: "Source code on GitHub",
                "GitHub"
            }
            button {
                class: if challenges.is_empty() { "ghost" } else { "ghost has-badge" },
                id: "notifications-button",
                title: if challenges.is_empty() {
                    "No challenges waiting for you".to_string()
                } else {
                    format!("{} challenge(s) waiting for you", challenges.len())
                },
                onclick: move |_| show_notifications.set(true),
                span { "\u{1F514}" }
                if !challenges.is_empty() {
                    span { class: "badge count", id: "notifications-count", "{challenges.len()}" }
                }
            }
            if show_admin_entry {
                // The label is deliberately fixed. It used to gain the word
                // "admin" once adminship resolved, which changed the button's
                // width and shifted everything beside it — the header visibly
                // jumped the moment the lobby arrived. Adminship goes in the
                // tooltip instead.
                button {
                    class: "ghost",
                    id: "admin-button",
                    title: if is_admin { "Administration (you are an admin)" } else { "Administration" },
                    onclick: move |_| show_admin.set(true),
                    "\u{2699}"
                }
            }
            button {
                class: "ghost",
                id: "account-button",
                onclick: move |_| show_account.set(true),
                "{nickname} ({rating})"
            }
        }
        if show_account() {
            AccountModal { state, sync, on_close: move |_| show_account.set(false) }
        }
        if show_admin() {
            AdminModal { state, sync, on_close: move |_| show_admin.set(false) }
        }
        if show_notifications() {
            NotificationsModal {
                state,
                sync,
                on_close: move |_| show_notifications.set(false),
            }
        }
    }
}

/// Challenges addressed to this player, reachable from the bell in the header.
///
/// A direct challenge is the one thing on FreeChess that is aimed at *you* and
/// then waits. An open game sits in a list anyone browses; a challenge is
/// addressed, and if you never happen to scroll past the "Challenges for you"
/// panel, nothing tells you it exists. Hence a count in the header, visible
/// from every page.
#[component]
pub fn NotificationsModal(
    state: Signal<AppState>,
    sync: Sync,
    on_close: EventHandler<()>,
) -> Element {
    let s = state.read();
    let me = s.me();
    let challenges: Vec<_> = s
        .lobby
        .games
        .challenges_for(me)
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    // Resolve by id from presence, so a challenger who has since renamed shows
    // under their current name rather than the one frozen into their signature.
    let named: Vec<(GameId, String, String)> = challenges
        .iter()
        .map(|e| {
            (
                e.game_id,
                s.lobby
                    .display_name(e.creator_id(), &e.setup.setup.creator_nickname),
                e.time_control().label(),
            )
        })
        .collect();
    drop(s);

    rsx! {
        div { class: "modal-bg", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |e| e.stop_propagation(),
                h3 { "Notifications" }
                div { class: "body",
                    if named.is_empty() {
                        div { class: "empty", id: "no-notifications",
                            "Nothing waiting for you right now."
                        }
                    } else {
                        div { class: "small muted", style: "margin-bottom:8px",
                            "Players who have challenged you directly. Only you can take these seats."
                        }
                        div { class: "list",
                            for (id, name, tc) in named {
                                div { key: "{id}", id: "notification-{id}", class: "list-item",
                                    span { class: "grow",
                                        b { "{name}" }
                                        span { class: "muted small", " challenged you " }
                                        span { class: "badge", "{tc}" }
                                    }
                                    button {
                                        class: "primary small",
                                        onclick: move |_| {
                                            sync.send(Cmd::JoinGame(id));
                                            on_close.call(());
                                        },
                                        "Play"
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "actions",
                    button { class: "ghost", onclick: move |_| on_close.call(()), "Close" }
                }
            }
        }
    }
}

/// Nickname, and importing/exporting the account key.
#[component]
pub fn AccountModal(state: Signal<AppState>, sync: Sync, on_close: EventHandler<()>) -> Element {
    let account = state.read().account.clone();
    let mut nickname = use_signal(|| account.nickname.clone());
    let mut import_text = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut revealed = use_signal(|| false);
    let export_string = account.export();
    let player_id = account.id();

    rsx! {
        div { class: "modal-bg", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |e| e.stop_propagation(),
                h3 { "Your account" }
                div { class: "body",
                    div {
                        label { "Nickname" }
                        input {
                            value: "{nickname}",
                            oninput: move |e| nickname.set(e.value()),
                            maxlength: 32,
                        }
                        div { class: "small muted", style: "margin-top:6px",
                            "Player id: "
                            span { class: "mono", "{player_id}" }
                        }
                    }
                    if let Some(err) = error() {
                        div { class: "banner err small", "{err}" }
                    }
                    div { class: "actions",
                        button {
                            class: "primary",
                            onclick: move |_| {
                                let value = nickname();
                                match validate_nickname(&value) {
                                    Ok(()) => {
                                        sync.send(Cmd::SetNickname(value));
                                        error.set(None);
                                        on_close.call(());
                                    }
                                    Err(e) => error.set(Some(e)),
                                }
                            },
                            "Save"
                        }
                    }

                    hr { style: "border:none;border-top:1px solid var(--border)" }

                    div {
                        label { "Export account" }
                        div { class: "small muted", style: "margin-bottom:6px",
                            "This key IS your account — anyone holding it can play as you. There is no way to reset it."
                        }
                        if revealed() {
                            code { class: "key", "{export_string}" }
                        } else {
                            button { class: "ghost", onclick: move |_| revealed.set(true), "Reveal key" }
                        }
                    }

                    div {
                        label { "Import an account" }
                        input {
                            value: "{import_text}",
                            placeholder: "fchess1...",
                            oninput: move |e| import_text.set(e.value()),
                        }
                        div { class: "actions", style: "margin-top:8px",
                            button {
                                class: "ghost",
                                onclick: move |_| {
                                    match account::import(&import_text()) {
                                        Ok(key) => {
                                            crate::identity::save_local(&key);
                                            // A full reload is the honest way to
                                            // swap identity: every subscription,
                                            // cached game and signature context
                                            // belongs to the old key.
                                            if let Some(w) = web_sys::window() {
                                                let _ = w.location().reload();
                                            }
                                        }
                                        Err(e) => error.set(Some(e.to_string())),
                                    }
                                },
                                "Import and reload"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Create-a-game dialog.
#[component]
pub fn NewGameModal(
    sync: Sync,
    #[props(default = None)] challenged: Option<(String, ed25519_dalek::VerifyingKey)>,
    on_close: EventHandler<()>,
) -> Element {
    let mut minutes = use_signal(|| 10u32);
    let mut increment = use_signal(|| 0u32);
    let mut color = use_signal(|| Color::White);
    let challenge_name = challenged.as_ref().map(|(n, _)| n.clone());
    let challenge_key = challenged.as_ref().map(|(_, k)| *k);

    let tc = TimeControl {
        initial_secs: minutes() * 60,
        increment_secs: increment(),
    };

    rsx! {
        div { class: "modal-bg", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |e| e.stop_propagation(),
                h3 {
                    if let Some(name) = challenge_name.clone() {
                        "Challenge {name}"
                    } else {
                        "New game"
                    }
                }
                div { class: "body",
                    div {
                        label { "Time control" }
                        div { class: "row",
                            for preset in [(1u32, 0u32), (3, 2), (5, 0), (10, 0), (15, 10), (30, 0)] {
                                button {
                                    key: "{preset.0}-{preset.1}",
                                    class: if minutes() == preset.0 && increment() == preset.1 { "primary" } else { "ghost" },
                                    onclick: move |_| { minutes.set(preset.0); increment.set(preset.1); },
                                    "{preset.0}+{preset.1}"
                                }
                            }
                        }
                        div { class: "small muted", style: "margin-top:8px",
                            span { class: "badge {tc.category().to_lowercase()}", "{tc.category()}" }
                        }
                    }
                    div {
                        label { "Play as" }
                        div { class: "row",
                            button {
                                class: if color() == Color::White { "primary" } else { "ghost" },
                                onclick: move |_| color.set(Color::White),
                                "White"
                            }
                            button {
                                class: if color() == Color::Black { "primary" } else { "ghost" },
                                onclick: move |_| color.set(Color::Black),
                                "Black"
                            }
                        }
                    }
                    div { class: "actions",
                        button { class: "ghost", onclick: move |_| on_close.call(()), "Cancel" }
                        button {
                            class: "primary",
                            onclick: move |_| {
                                sync.send(Cmd::CreateGame {
                                    color: color(),
                                    time_control: tc,
                                    challenged: challenge_key,
                                });
                                on_close.call(());
                            },
                            if challenge_name.is_some() { "Send challenge" } else { "Create game" }
                        }
                    }
                }
            }
        }
    }
}

/// The home page: every game in progress, plus open challenges, ranking and
/// who is around.
#[component]
pub fn HomeView(state: Signal<AppState>, sync: Sync) -> Element {
    let mut show_new = use_signal(|| false);
    let mut challenge_target = use_signal(|| None::<(String, ed25519_dalek::VerifyingKey)>);
    let mut query = use_signal(String::new);
    let mut tab = use_signal(|| "live");

    let s = state.read();
    let me = s.me();
    let now = now_ms();
    let search = query();
    let searching = !search.trim().is_empty();
    let accepting_new_games = s.lobby.administration.accepting_new_games();

    let mut live: Vec<_> = if searching {
        s.lobby.games.search(&search).into_iter().cloned().collect()
    } else {
        s.lobby.games.live_games().into_iter().cloned().collect()
    };
    // A takedown is advisory, and this is what honouring it looks like: the
    // game stops appearing where clients look.
    live.retain(|e| !s.lobby.administration.is_taken_down(&e.game_id));
    let finished: Vec<_> = s
        .lobby
        .games
        .finished_games()
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    // Names resolved by id, so a rename shows up everywhere immediately.
    let name_of = |e: &LobbyEntry| s.lobby.game_names(e);
    let live_named: Vec<_> = live.iter().map(|e| (e.clone(), name_of(e))).collect();
    let finished_named: Vec<_> = finished.iter().map(|e| (e.clone(), name_of(e))).collect();
    let open: Vec<_> = s
        .lobby
        .games
        .public_open_games()
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    let invites: Vec<_> = s
        .lobby
        .games
        .challenges_for(me)
        .into_iter()
        .cloned()
        .collect();
    let ranked: Vec<_> = s.lobby.leaderboard.ranked().into_iter().cloned().collect();
    let players: Vec<_> = s
        .lobby
        .presence
        .challengeable(me, now)
        .into_iter()
        .cloned()
        .collect();
    drop(s);

    rsx! {
        div { class: "page",
            div { class: "columns",
                div {
                    div { class: "row", style: "margin-bottom:16px",
                        input {
                            placeholder: "Search games by player or id\u{2026}",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                        // Disabled while the app is migrating. The lobby also
                        // refuses the listing, so this is the honest signal
                        // rather than the enforcement — a button that appears
                        // to work and then silently drops the game would be
                        // worse than one that says why it is off.
                        button {
                            class: "primary",
                            style: "white-space:nowrap",
                            disabled: !accepting_new_games,
                            title: if accepting_new_games { "" } else { "FreeChess has moved — new games are closed at this address" },
                            onclick: move |_| show_new.set(true),
                            "New game"
                        }
                    }

                    if !invites.is_empty() {
                        div { class: "panel",
                            h2 { "Challenges for you" }
                            div { class: "list",
                                for entry in invites {
                                    div { key: "{entry.game_id}", class: "list-item",
                                        span { class: "grow",
                                            b { "{entry.setup.setup.creator_nickname}" }
                                            span { class: "muted small", " wants to play " }
                                            span { class: "badge {entry.time_control().category().to_lowercase()}",
                                                "{entry.time_control().label()}"
                                            }
                                        }
                                        button {
                                            class: "primary",
                                            onclick: {
                                                let id = entry.game_id;
                                                move |_| sync.send(Cmd::JoinGame(id))
                                            },
                                            "Accept"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "panel",
                        if searching {
                            h2 {
                                "Search results"
                                span { class: "badge live", "{live_named.len()}" }
                            }
                        } else {
                            div { class: "tabs",
                                button {
                                    class: if tab() == "live" { "active" } else { "" },
                                    onclick: move |_| tab.set("live"),
                                    "In progress ({live_named.len()})"
                                }
                                button {
                                    class: if tab() == "finished" { "active" } else { "" },
                                    onclick: move |_| tab.set("finished"),
                                    "Finished ({finished_named.len()})"
                                }
                            }
                        }

                        {
                            // Searching cuts across both, so it shows its own results.
                            let showing = if searching || tab() == "live" {
                                live_named.clone()
                            } else {
                                finished_named.clone()
                            };
                            let is_finished_tab = !searching && tab() == "finished";

                            if showing.is_empty() {
                                rsx! {
                                    div { class: "empty",
                                        if searching { "Nothing matched that search." }
                                        else if is_finished_tab { "No finished games yet." }
                                        else { "No games running yet — start one and it will show up here for everyone." }
                                    }
                                }
                            } else {
                                rsx! {
                                    div { class: "games",
                                        for (entry, (white, black)) in showing {
                                            {
                                                let id = entry.game_id;
                                                let tc = entry.time_control();
                                                let over = entry.result().is_over();
                                                let class = if over { "game-card finished" } else { "game-card" };
                                                rsx! {
                                                    div {
                                                        key: "{id}",
                                                        class: "{class}",
                                                        onclick: move |_| {
                                                            sync.send(Cmd::WatchGame(id));
                                                            state.with_mut(|s| s.route = Route::Game(id));
                                                        },
                                                        div { class: "mini",
                                                            MiniBoard { fen: entry.fen(), orientation: Color::White }
                                                        }
                                                        div { class: "who",
                                                            b { "{white}" }
                                                            span { class: "muted", "vs" }
                                                            b { "{black}" }
                                                        }
                                                        div { class: "meta",
                                                            span { class: "badge {tc.category().to_lowercase()}", "{tc.label()}" }
                                                            if over {
                                                                span { class: "result-line", "{short_result(entry.result())}" }
                                                            } else {
                                                                span { "move {entry.snapshot.as_ref().map(|s| s.ply / 2 + 1).unwrap_or(1)}" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "panel",
                        h2 { "Open challenges" }
                        if open.is_empty() {
                            div { class: "empty", "Nobody is waiting for an opponent right now." }
                        } else {
                            div { class: "list",
                                for entry in open {
                                    // The id is here so a test can address one
                                    // specific listing: several open games
                                    // usually share a creator name, so picking
                                    // by name gets an arbitrary one of them.
                                    div { key: "{entry.game_id}", id: "listing-{entry.game_id}", class: "list-item",
                                        span { class: "grow",
                                            b { "{entry.setup.setup.creator_nickname}" }
                                        }
                                        span { class: "badge {entry.time_control().category().to_lowercase()}",
                                            "{entry.time_control().label()}"
                                        }
                                        if entry.creator_id() == me {
                                            span { class: "muted small", "yours" }
                                        } else {
                                            button {
                                                class: "primary",
                                                onclick: {
                                                    let id = entry.game_id;
                                                    move |_| sync.send(Cmd::JoinGame(id))
                                                },
                                                "Play"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div {
                    div { class: "panel",
                        h2 { "Ranking" }
                        if ranked.is_empty() {
                            div { class: "empty", "No rated games yet." }
                        } else {
                            div { class: "list",
                                for (i, entry) in ranked.iter().take(20).enumerate() {
                                    div { key: "{entry.player_id()}", class: "list-item",
                                        span { class: "rank", "{i + 1}" }
                                        span { class: "grow", "{entry.nickname}" }
                                        span { class: "elo", "{entry.rating}" }
                                    }
                                }
                            }
                        }
                    }

                    // Identified so a test can address this panel specifically:
                    // player names appear in the ranking and in game listings
                    // too, and matching on a name alone picks whichever comes
                    // first — a row with no Challenge button on it.
                    div { class: "panel", id: "players-panel",
                        h2 { "Players" }
                        if players.is_empty() {
                            div { class: "empty", "Nobody else is online." }
                        } else {
                            div { class: "list",
                                for p in players {
                                    div { key: "{p.player_id()}", class: "list-item",
                                        span { class: "dot {p.status_at(now).label()}" }
                                        span { class: "grow", "{p.nickname}" }
                                        button {
                                            class: "ghost small",
                                            onclick: {
                                                let name = p.nickname.clone();
                                                let key = p.player;
                                                move |_| challenge_target.set(Some((name.clone(), key)))
                                            },
                                            "Challenge"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if show_new() {
            NewGameModal { sync, on_close: move |_| show_new.set(false) }
        }
        if let Some(target) = challenge_target() {
            NewGameModal {
                sync,
                challenged: Some(target),
                on_close: move |_| challenge_target.set(None),
            }
        }
    }
}

/// A single game — playing or spectating, plus the replay scrubber once it is
/// over.
#[component]
pub fn GameView(
    state: Signal<AppState>,
    sync: Sync,
    game_id: GameId,
    replay_mode: bool,
) -> Element {
    let mut promotion = use_signal(|| None::<(Move, Vec<PieceKind>)>);
    let mut viewing_ply = use_signal(|| None::<usize>);
    let mut challenge_target = use_signal(|| None::<(String, ed25519_dalek::VerifyingKey)>);

    // Tick once a second so the clocks actually count down. Everything else in
    // this view is driven by state changes, and between moves there are none —
    // without this the clock only moved when an update happened to arrive.
    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            crate::gloo_sleep(1000).await;
            tick += 1;
        }
    });
    // Read it so this component actually subscribes to the tick.
    let _ = tick();

    let s = state.read();
    let me = s.me();
    let now = now_ms();
    let game = match s.game(&game_id) {
        Some(g) => g.clone(),
        None => {
            drop(s);
            return rsx! {
                div { class: "page",
                    div { class: "panel", div { class: "empty", "Loading game\u{2026}" } }
                }
            };
        }
    };
    let spectators: Vec<_> = s
        .lobby
        .presence
        .spectators_of(game_id, now)
        .into_iter()
        .filter(|p| p.player_id() != me)
        .cloned()
        .collect();
    drop(s);

    let replay = game.replay();
    let my_color = game
        .player_keys()
        .and_then(|_| game.color_of(&state.read().account.key.verifying_key()));
    let orientation = my_color.unwrap_or(Color::White);
    let result = game.result();
    let is_over = result.is_over();

    // In replay mode (or when scrubbing) the board shows a past position and is
    // not playable.
    let total_plies = replay.ply();
    let shown_ply = viewing_ply().unwrap_or(total_plies).min(total_plies);
    let scrubbing = shown_ply != total_plies;
    let fen = replay.fen_at(shown_ply).to_string();

    let playable = if replay_mode || scrubbing || is_over {
        None
    } else {
        my_color.filter(|c| game.replay().side_to_move() == *c && game.opponent.get().is_some())
    };

    let last_move = if shown_ply > 0 {
        replay.moves().get(shown_ply - 1).copied()
    } else {
        None
    };

    // Resolved by id from presence, so a rename shows immediately here too; the
    // signed copy in the setup/join is the fallback.
    let (white_name, black_name) = match game.setup.get() {
        Some(setup) => {
            let creator_embedded = setup.setup.creator_nickname.clone();
            let challenger_embedded = game
                .opponent
                .get()
                .map(|j| j.nickname.clone())
                .unwrap_or_else(|| "waiting\u{2026}".to_string());
            let s = state.read();
            let creator = s
                .lobby
                .display_name(PlayerId::from(&setup.creator), &creator_embedded);
            let challenger = match game.opponent.get() {
                Some(j) => s.lobby.display_name(j.player_id(), &challenger_embedded),
                None => challenger_embedded,
            };
            drop(s);
            match setup.setup.creator_plays {
                Color::White => (creator, challenger),
                Color::Black => (challenger, creator),
            }
        }
        None => ("White".to_string(), "Black".to_string()),
    };

    let board_now = Board::from_fen(&fen).unwrap_or_default();
    let (captured_by_white, captured_by_black) = captured_material(&board_now);
    // Freeze the clocks at the moment the game ended. `time_remaining` measures
    // against "now" and judges liveness from the BOARD alone (so that a timeout
    // claim can be verified against the state that contains it) — which means a
    // resignation or agreed draw leaves the mover's clock still ticking.
    let clock_now = if is_over {
        game.conclusion
            .effective(&game)
            .map(|c| c.at)
            .or_else(|| game.moves.timestamps().last().copied())
            .unwrap_or(now)
    } else {
        now
    };
    let white_ms = game.time_remaining(Color::White, clock_now);
    let black_ms = game.time_remaining(Color::Black, clock_now);
    let to_move = replay.side_to_move();

    // A flag fall decides nothing on its own — someone has to claim it. The
    // claim only holds once the opponent's *own* attestations put the deadline
    // in the past, so offer it exactly when it would be accepted.
    let timeout_claimable = my_color
        .filter(|_| !is_over)
        .and_then(|mine| {
            let at_ply = game.moves.move_list().len() as u32;
            game.timeout_provable_at(mine.opposite(), at_ply)
        })
        .map(|provable| now >= provable)
        .unwrap_or(false);

    let sans = replay.san_list();
    // Our offer is on the table but the creator has not countersigned it yet.
    // Until they do the seat is empty and no move is legal — see
    // `chess_core::game::opponent`.
    let offer_pending = game.opponent.get().is_none()
        && game
            .opponent
            .pending_offers()
            .iter()
            .any(|j| j.player_id() == me);
    let can_join = !offer_pending
        && game.is_open()
        && game
            .setup
            .get()
            .map(|s| s.creator)
            .map(|c| PlayerId::from(&c))
            != Some(me)
        && game
            .setup
            .get()
            .and_then(|s| s.setup.challenged)
            .map(|k| PlayerId::from(&k) == me)
            .unwrap_or(true);

    let seat = |name: String, ms: i64, color: Color, captured: String| {
        let active = !is_over && to_move == color && game.opponent.get().is_some();
        let low = ms < 20_000;
        let mut class = String::from("clock");
        if active {
            class.push_str(" active");
        }
        if active && low {
            class.push_str(" low");
        }
        let piece_class = if color == Color::White {
            "piece white"
        } else {
            "piece black"
        };
        rsx! {
            div { class: "seat",
                span { class: "{piece_class}", style: "font-size:20px", "\u{265F}" }
                span { class: "name", "{name}" }
                span { class: "captured", "{captured}" }
                span { class: "spacer" }
                span { class: "{class}", "{format_clock(ms)}" }
            }
        }
    };

    rsx! {
        div { class: "page",
            div { class: "game-layout",
                div {
                    { seat(black_name.clone(), black_ms, Color::Black, captured_by_black) }
                    div { class: "board-wrap",
                        GameBoard {
                            fen: fen.clone(),
                            orientation,
                            playable,
                            last_move,
                            on_move: move |mv: Move| {
                                // A pawn reaching the last rank needs a piece
                                // chosen before the move can be signed.
                                let board = Board::from_fen(&fen).unwrap_or_default();
                                let is_promo = board
                                    .piece_at(mv.from)
                                    .map(|p| p.kind == PieceKind::Pawn
                                        && mv.to.rank() == p.color.promotion_rank())
                                    .unwrap_or(false);
                                if is_promo {
                                    promotion.set(Some((mv, vec![
                                        PieceKind::Queen, PieceKind::Rook,
                                        PieceKind::Bishop, PieceKind::Knight,
                                    ])));
                                } else {
                                    sync.send(Cmd::PlayMove { game: game_id, mv });
                                }
                            },
                        }
                        if is_over && !scrubbing {
                            div { class: "result-overlay",
                                div { class: "result-card",
                                    div { class: "headline", "{result_headline(result, my_color)}" }
                                    div { class: "muted small", "Replay is shareable — copy the page link." }
                                }
                            }
                        }
                    }
                    { seat(white_name.clone(), white_ms, Color::White, captured_by_white) }
                }

                div {
                    div { class: "panel",
                        h2 { "Moves" }
                        div { class: "moves",
                            if sans.is_empty() {
                                div { class: "empty", "No moves yet." }
                            } else {
                                for (i, pair) in sans.chunks(2).enumerate() {
                                    div { key: "{i}", class: "mv-row",
                                        span { class: "num", "{i + 1}." }
                                        span {
                                            class: if shown_ply == i * 2 + 1 { "mv current" } else { "mv" },
                                            onclick: move |_| viewing_ply.set(Some(i * 2 + 1)),
                                            "{pair[0]}"
                                        }
                                        if pair.len() > 1 {
                                            span {
                                                class: if shown_ply == i * 2 + 2 { "mv current" } else { "mv" },
                                                onclick: move |_| viewing_ply.set(Some(i * 2 + 2)),
                                                "{pair[1]}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "body",
                            div { class: "replay-bar",
                                button { class: "ghost", onclick: move |_| viewing_ply.set(Some(0)), "\u{23EE}" }
                                button {
                                    class: "ghost",
                                    onclick: move |_| viewing_ply.set(Some(shown_ply.saturating_sub(1))),
                                    "\u{25C0}"
                                }
                                button {
                                    class: "ghost",
                                    onclick: move |_| viewing_ply.set(Some((shown_ply + 1).min(total_plies))),
                                    "\u{25B6}"
                                }
                                button { class: "ghost", onclick: move |_| viewing_ply.set(None), "\u{23ED}" }
                            }
                            if scrubbing {
                                div {
                                    class: "banner warn small",
                                    id: "replay-position",
                                    style: "margin-top:10px",
                                    "Viewing move {shown_ply} of {total_plies}. "
                                    span {
                                        style: "cursor:pointer;text-decoration:underline",
                                        onclick: move |_| viewing_ply.set(None),
                                        "Jump to live"
                                    }
                                }
                            }
                        }
                    }

                    div { class: "panel",
                        h2 { "Game" }
                        div { class: "body",
                            div { class: "small muted", style: "margin-bottom:10px",
                                if my_color.is_some() { "You are playing this game." }
                                else { "You are spectating." }
                            }
                            if can_join {
                                button {
                                    class: "primary",
                                    style: "width:100%",
                                    onclick: move |_| sync.send(Cmd::JoinGame(game_id)),
                                    "Take the open seat"
                                }
                            }
                            if offer_pending {
                                div { class: "small muted",
                                    "Waiting for the creator to confirm you as their opponent\u{2026}"
                                }
                            }
                            if my_color.is_some() && !is_over && game.opponent.get().is_some() {
                                button {
                                    class: "danger",
                                    style: "width:100%;margin-top:8px",
                                    onclick: move |_| sync.send(Cmd::Resign(game_id)),
                                    "Resign"
                                }
                            }
                            if timeout_claimable {
                                button {
                                    class: "primary",
                                    style: "width:100%;margin-top:8px",
                                    onclick: move |_| sync.send(Cmd::ClaimTimeout(game_id)),
                                    "Claim the win on time"
                                }
                            }
                            div { class: "small muted", style: "margin-top:12px",
                                "Share this game"
                                // The address bar cannot be updated from here:
                                // the gateway runs the app in a sandboxed
                                // iframe without `allow-same-origin`, so
                                // history.replaceState is not permitted. The
                                // shell does forward its query string into the
                                // frame, so a link built here works when
                                // opened — it just has to be shown rather than
                                // reflected in the URL.
                                code {
                                    class: "key",
                                    id: "share-link",
                                    "{share_link(game_id)}"
                                }
                            }
                        }
                    }

                    div { class: "panel",
                        h2 { "Spectators" }
                        if spectators.is_empty() {
                            div { class: "empty", "Nobody else is watching." }
                        } else {
                            div { class: "list",
                                for p in spectators {
                                    div { key: "{p.player_id()}", class: "list-item",
                                        span { class: "dot {p.status_at(now).label()}" }
                                        span { class: "grow", "{p.nickname}" }
                                        button {
                                            class: "ghost small",
                                            onclick: {
                                                let name = p.nickname.clone();
                                                let key = p.player;
                                                move |_| challenge_target.set(Some((name.clone(), key)))
                                            },
                                            "Challenge"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some((mv, choices)) = promotion() {
            div { class: "modal-bg",
                div { class: "modal", style: "max-width:320px",
                    h3 { "Promote to" }
                    div { class: "body",
                        div { class: "promo",
                            for kind in choices {
                                button {
                                    key: "{kind:?}",
                                    onclick: move |_| {
                                        sync.send(Cmd::PlayMove {
                                            game: game_id,
                                            mv: Move::promoting(mv.from, mv.to, kind),
                                        });
                                        promotion.set(None);
                                    },
                                    "{crate::board::glyph(kind)}"
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(target) = challenge_target() {
            NewGameModal {
                sync,
                challenged: Some(target),
                on_close: move |_| challenge_target.set(None),
            }
        }
    }
}

/// Banner shown when the node connection is not healthy.
#[component]
pub fn StatusBanner(state: Signal<AppState>) -> Element {
    let s = state.read();
    let message = s.message.clone();
    let status = s.status;
    drop(s);

    rsx! {
        if status == crate::app::ConnStatus::Error {
            div { class: "page", style: "padding-bottom:0",
                div { class: "banner err",
                    b { "Cannot reach the Freenet node." }
                    div { class: "small muted",
                        "This app talks to the node that served it. Check that the node is running."
                    }
                    if let Some(msg) = message.clone() {
                        div { class: "small mono muted", id: "app-message", style: "margin-top:6px", "{msg}" }
                    }
                }
            }
        } else if let Some(msg) = message.clone() {
            // Surface non-fatal problems too. A silently swallowed error here
            // reads as "the app just does not work", which is far harder to
            // diagnose than a visible line of text.
            div { class: "page", style: "padding-bottom:0",
                div { class: "banner warn small mono", id: "app-message", "{msg}" }
            }
        }
    }
}

/// The URL that opens this game for someone else.
///
/// Built from the frame's own location minus the shell's `__sandbox` marker.
/// The gateway shell forwards any extra query parameters into the iframe, so a
/// link of this shape lands the visitor straight in the game.
pub fn share_link(game_id: GameId) -> String {
    let fallback = format!("?game={game_id}");
    let Some(window) = web_sys::window() else {
        return fallback;
    };
    let location = window.location();
    let (Ok(origin), Ok(pathname)) = (location.origin(), location.pathname()) else {
        return fallback;
    };
    // A sandboxed frame reports its origin as "null"; fall back to a relative
    // link rather than emitting a nonsense absolute URL.
    if origin == "null" || origin.is_empty() {
        return format!("{pathname}?game={game_id}");
    }
    format!("{origin}{pathname}?game={game_id}")
}

/// Notices every visitor sees: the service state and any announcements.
///
/// Both are advisory by design — nothing in a contract refuses work because of
/// them (see `chess_core::admin`), so the honest presentation is a notice
/// rather than a blocked screen.
#[component]
pub fn NoticeBanners(state: Signal<AppState>) -> Element {
    let s = state.read();
    let unavailable = !s.lobby.administration.available();
    let service_message = s.lobby.administration.service_message();
    let migration = s.lobby.administration.migration().cloned();
    let announcements: Vec<_> = s
        .lobby
        .administration
        .recent_announcements()
        .into_iter()
        .take(3)
        .cloned()
        .collect();
    drop(s);

    if !unavailable && announcements.is_empty() && migration.is_none() {
        return rsx! {};
    }

    // The new address is a contract id, and the contract validated it as one —
    // 32 bytes of base58 — so building a URL from it cannot inject anything.
    // The origin comes from the page rather than being hardcoded, because the
    // app is served BY the node, on whatever port that node listens on.
    let new_url = migration.as_ref().map(|m| {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}/v1/contract/web/{}/", m.new_address)
    });

    rsx! {
        div { class: "page", style: "padding-bottom:0",
            if let (Some(m), Some(url)) = (migration.clone(), new_url) {
                div { class: "banner warn", id: "migration-notice",
                    b { "FreeChess has moved to a new address." }
                    div { class: "small", style: "margin-top:4px",
                        "New games are closed here. Games already in progress keep running, "
                        "and you can finish them on this page."
                    }
                    if !m.message.trim().is_empty() {
                        div { class: "small", style: "margin-top:4px", "{m.message}" }
                    }
                    div { class: "small mono", id: "migration-new-address", style: "margin-top:6px;word-break:break-all",
                        "{url}"
                    }
                }
            }
            if unavailable {
                div { class: "banner err", id: "service-notice",
                    b { "FreeChess is marked unavailable." }
                    if let Some(msg) = service_message {
                        div { class: "small", style: "margin-top:4px", "{msg}" }
                    }
                }
            }
            for a in announcements {
                div { key: "{a.id:?}", class: "banner warn", style: "margin-top:8px",
                    div { class: "small", "{a.text}" }
                }
            }
        }
    }
}

/// The administration panel.
///
/// Shown to admins, and to anyone at all while adminship is unclaimed — which
/// is what "first to claim it" means in practice, and is deliberately visible
/// rather than hidden, since it is a race anyone can win.
#[component]
pub fn AdminModal(state: Signal<AppState>, sync: Sync, on_close: EventHandler<()>) -> Element {
    let mut announcement = use_signal(String::new);
    let mut service_message = use_signal(String::new);
    let mut migration_address = use_signal(String::new);
    let mut migration_message = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let s = state.read();
    let me = s.me();
    let is_admin = s.is_admin();
    let unclaimed = s.admin_unclaimed();
    let available = s.lobby.administration.available();
    let current_migration = s.lobby.administration.migration().cloned();
    let admins: Vec<_> = s
        .lobby
        .administration
        .admins
        .listed()
        .into_iter()
        .cloned()
        .collect();
    let now = now_ms();
    // Candidates to promote: anyone present who is not already an admin.
    let candidates: Vec<_> = s
        .lobby
        .presence
        .active(now)
        .into_iter()
        .filter(|p| !s.lobby.administration.is_admin(p.player_id()) && p.player_id() != me)
        .cloned()
        .collect();
    let live: Vec<_> = s
        .lobby
        .games
        .live_games()
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    drop(s);

    rsx! {
        div { class: "modal-bg", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |e| e.stop_propagation(),
                h3 { "Administration" }
                div { class: "body",
                    if let Some(err) = error() {
                        div { class: "banner err small", "{err}" }
                    }

                    if !is_admin {
                        if unclaimed {
                            div {
                                div { class: "banner warn small",
                                    b { "Nobody has claimed adminship yet." }
                                    div { style: "margin-top:4px",
                                        "The first claim wins, and anyone can make it \u{2014} including someone else, at any moment."
                                    }
                                }
                                button {
                                    class: "primary",
                                    id: "claim-admin",
                                    style: "width:100%;margin-top:10px",
                                    onclick: move |_| {
                                        sync.send(Cmd::ClaimAdmin);
                                        error.set(None);
                                    },
                                    "Claim adminship"
                                }
                            }
                        } else {
                            div { class: "banner small", "You are not an administrator." }
                        }
                    }

                    div {
                        label { "Administrators ({admins.len()})" }
                        if admins.is_empty() {
                            div { class: "small muted", "None." }
                        } else {
                            div { class: "list",
                                for a in admins {
                                    div { key: "{a.grantee_id()}", class: "list-item",
                                        span { class: "grow", "{a.nickname}" }
                                        if a.is_self_claim() {
                                            span { class: "badge", "root" }
                                        }
                                        if a.grantee_id() == me {
                                            span { class: "badge rapid", "you" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if is_admin {
                        div {
                            label { "Promote a player" }
                            if candidates.is_empty() {
                                div { class: "small muted", "Nobody else is online." }
                            } else {
                                div { class: "list",
                                    for p in candidates {
                                        div { key: "{p.player_id()}", class: "list-item",
                                            span { class: "grow", "{p.nickname}" }
                                            button {
                                                class: "ghost small",
                                                onclick: {
                                                    let player = p.player;
                                                    let nick = p.nickname.clone();
                                                    move |_| sync.send(Cmd::GrantAdmin {
                                                        player,
                                                        nickname: nick.clone(),
                                                    })
                                                },
                                                "Make admin"
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div {
                            label { "Publish an announcement" }
                            input {
                                id: "announcement-text",
                                value: "{announcement}",
                                placeholder: "Shown to everyone\u{2026}",
                                maxlength: 500,
                                oninput: move |e| announcement.set(e.value()),
                            }
                            div { class: "actions", style: "margin-top:8px",
                                button {
                                    class: "primary",
                                    id: "publish-announcement",
                                    onclick: move |_| {
                                        let text = announcement();
                                        if text.trim().is_empty() {
                                            error.set(Some("the announcement is empty".to_string()));
                                        } else {
                                            sync.send(Cmd::Announce { text });
                                            announcement.set(String::new());
                                            error.set(None);
                                        }
                                    },
                                    "Announce"
                                }
                            }
                        }

                        div {
                            label { "Announce a move to a new address" }
                            div { class: "small muted", style: "margin-bottom:6px",
                                "Use this when the app has been republished at a different contract "
                                "address. Everyone still on this address is told where to go, and "
                                "the lobby stops accepting new games. Games already in progress are "
                                "left alone and can be finished here."
                            }
                            if let Some(m) = current_migration.clone() {
                                div { class: "small mono", style: "margin-bottom:6px;word-break:break-all",
                                    "Currently announced: {m.new_address}"
                                }
                            }
                            input {
                                id: "migration-address",
                                value: "{migration_address}",
                                placeholder: "New contract address (base58)\u{2026}",
                                oninput: move |e| migration_address.set(e.value()),
                            }
                            input {
                                id: "migration-message",
                                style: "margin-top:6px",
                                value: "{migration_message}",
                                placeholder: "Optional note\u{2026}",
                                maxlength: 500,
                                oninput: move |e| migration_message.set(e.value()),
                            }
                            div { class: "actions", style: "margin-top:8px",
                                button {
                                    class: "danger",
                                    id: "announce-migration",
                                    onclick: move |_| {
                                        let addr = migration_address().trim().to_string();
                                        if addr.is_empty() {
                                            error.set(Some("enter the new contract address".to_string()));
                                        } else if chess_core::identity::GameId::from_base58(&addr).is_none() {
                                            // Caught here as well as in the
                                            // command, so the admin gets the
                                            // reason instead of a notice that
                                            // every peer quietly discards.
                                            error.set(Some("that is not a valid contract address".to_string()));
                                        } else {
                                            sync.send(Cmd::AnnounceMigration {
                                                new_address: addr,
                                                message: migration_message(),
                                            });
                                            migration_address.set(String::new());
                                            migration_message.set(String::new());
                                            error.set(None);
                                        }
                                    },
                                    "Announce the move"
                                }
                                if current_migration.is_some() {
                                    button {
                                        class: "ghost",
                                        id: "cancel-migration",
                                        onclick: move |_| {
                                            sync.send(Cmd::AnnounceMigration {
                                                new_address: String::new(),
                                                message: String::new(),
                                            });
                                            error.set(None);
                                        },
                                        "Call it off"
                                    }
                                }
                            }
                        }

                        div {
                            label { "End a game" }
                            div { class: "small muted", style: "margin-bottom:6px",
                                "Advisory: the game disappears from listings, but its two players can still finish it."
                            }
                            if live.is_empty() {
                                div { class: "small muted", "No games in progress." }
                            } else {
                                div { class: "list",
                                    for e in live {
                                        {
                                            let (white, black) = e.nicknames();
                                            let id = e.game_id;
                                            rsx! {
                                                div { key: "{id}", class: "list-item",
                                                    span { class: "grow small", "{white} vs {black}" }
                                                    button {
                                                        class: "danger small",
                                                        onclick: move |_| sync.send(Cmd::TakeDownGame {
                                                            game: id,
                                                            reason: "ended by an administrator".to_string(),
                                                        }),
                                                        "End"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div {
                            label { "Service state" }
                            div { class: "small muted", style: "margin-bottom:6px",
                                "Advisory: clients show a notice. Nothing is blocked, and a modified client can ignore it."
                            }
                            if available {
                                input {
                                    id: "service-message",
                                    value: "{service_message}",
                                    placeholder: "Reason shown to players\u{2026}",
                                    oninput: move |e| service_message.set(e.value()),
                                }
                                button {
                                    class: "danger",
                                    id: "mark-unavailable",
                                    style: "width:100%;margin-top:8px",
                                    onclick: move |_| {
                                        sync.send(Cmd::SetService {
                                            available: false,
                                            message: service_message(),
                                        });
                                        error.set(None);
                                    },
                                    "Mark unavailable"
                                }
                            } else {
                                button {
                                    class: "primary",
                                    id: "mark-available",
                                    style: "width:100%",
                                    onclick: move |_| {
                                        sync.send(Cmd::SetService {
                                            available: true,
                                            message: String::new(),
                                        });
                                        error.set(None);
                                    },
                                    "Mark available again"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Compact result label for a finished game in the lobby.
pub fn short_result(result: chess_core::game::GameResult) -> &'static str {
    use chess_core::game::GameResult::*;
    match result {
        WhiteWins(_) => "1-0",
        BlackWins(_) => "0-1",
        Draw(_) => "\u{00BD}-\u{00BD}",
        InProgress => "playing",
        AwaitingOpponent => "waiting",
    }
}

/// Asks for a nickname when neither the delegate nor the network knows one.
///
/// Only appears once both have had a chance to answer, so a returning player is
/// never asked again — the point is to stop generating a throwaway name on every
/// visit, not to add a step for people who already have one.
#[component]
pub fn ChooseNicknamePrompt(state: Signal<AppState>, sync: Sync) -> Element {
    let mut chosen = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // Only prompt once the delegate has answered; before that we do not yet
    // know whether this player already has a name.
    let mut waited = use_signal(|| false);

    let s = state.read();
    let needs_name = s.nickname_unset;
    let settled = s.account_settled;
    let player_id = s.me();
    drop(s);

    use_future(move || async move {
        crate::gloo_sleep(4000).await;
        waited.set(true);
    });

    if !needs_name || !settled || !waited() {
        return rsx! {};
    }

    rsx! {
        div { class: "modal-bg",
            div { class: "modal", style: "max-width:400px",
                h3 { "Choose a nickname" }
                div { class: "body",
                    div { class: "small muted",
                        "This is how other players will see you. You can change it later."
                    }
                    input {
                        id: "choose-nickname",
                        value: "{chosen}",
                        placeholder: "your name",
                        maxlength: 32,
                        autofocus: true,
                        oninput: move |e| chosen.set(e.value()),
                    }
                    div { class: "small muted",
                        "Your id: "
                        span { class: "mono", "{player_id}" }
                    }
                    if let Some(err) = error() {
                        div { class: "banner err small", "{err}" }
                    }
                    div { class: "actions",
                        button {
                            class: "primary",
                            id: "confirm-nickname",
                            onclick: move |_| {
                                let value = chosen();
                                match validate_nickname(&value) {
                                    Ok(()) => {
                                        sync.send(Cmd::SetNickname(value));
                                        error.set(None);
                                    }
                                    Err(e) => error.set(Some(e)),
                                }
                            },
                            "Continue"
                        }
                    }
                }
            }
        }
    }
}
