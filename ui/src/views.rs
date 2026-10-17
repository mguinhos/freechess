//! The screens.

use crate::app::{format_clock, result_headline, AppState, Cmd, ConnStatus, Route, Sync};
use crate::board::{captured_material, GameBoard, MiniBoard, PieceImage};
use crate::identity::now_ms;
use crate::theme::{PieceSet, Theme};
use chess_core::account;
use chess_core::chess::{Board, Color, Move, PieceKind};
use chess_core::game::setup::TimeControl;
use chess_core::identity::{GameId, PlayerId};
use chess_core::lobby::LobbyEntry;
use chess_core::player::validate_nickname;
use dioxus::prelude::*;

/// The release this page was built from.
///
/// `version` is the workspace version; `build` is the commit it came from,
/// exported by `cargo make build-ui`. A page that cannot say which build it is
/// makes every report of odd behaviour unattributable — which, during a run of
/// rapid republishes, is most of the difficulty.
///
/// River stamps theirs from `build.rs` instead, which holds however the UI is
/// built. That is nicer in principle and does not transfer here: their trick
/// depends on emitting NO `cargo:rerun-if-changed`, so Cargo's default re-run
/// heuristic keeps the stamp fresh — and ours must emit several, because it has
/// to re-run whenever a contract's WASM changes. With those present the script
/// would not re-run on a commit that touched no contract, and the stamp would
/// quietly go stale, which is worse than saying nothing.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD: &str = match option_env!("FREECHESS_BUILD") {
    Some(stamp) => stamp,
    // A plain `dx build`, outside the task that stamps it.
    None => "unstamped",
};
const PIECE_CREDITS: Asset = asset!("/assets/pieces/credits.html");

fn icon(name: &str) -> Element {
    let path = match name {
        "bell" => "M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4",
        "settings" => "M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1 1-3z M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
        "palette" => "M12 3C6 3 3 7 3 12s4 9 9 9h1a2 2 0 0 0 0-4h-1a1 1 0 0 1-1-1c0-1 1-2 3-2h2a5 5 0 0 0 5-5c0-3-4-6-9-6z M7 10h.01 M10 6h.01 M15 6h.01 M18 9h.01",
        "user" => "M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0 M4 21v-2a8 8 0 0 1 16 0v2",
        "play" => "m8 4 12 8-12 8V4z",
        "first" => "M5 4v16 M19 4 7 12l12 8V4z",
        "previous" => "m16 4-12 8 12 8V4z",
        "next" => "m8 4 12 8-12 8V4z",
        "last" => "M19 4v16 M5 4l12 8-12 8V4z",
        "flip" => "M4 8h15l-4-4 M20 16H5l4 4",
        "copy" => "M9 9h12v12H9V9z M15 5V3H3v12h2",
        "download" => "M12 3v12m-5-5 5 5 5-5 M4 16v5h16v-5",
        "sound" => "M3 9h4l5-5v16l-5-5H3V9z M16 8a6 6 0 0 1 0 8 M19 5a10 10 0 0 1 0 14",
        "muted" => "M3 9h4l5-5v16l-5-5H3V9z M16 9l6 6m0-6-6 6",
        "close" => "m6 6 12 12 M6 18 18 6",
        "home" => "m3 11 9-8 9 8 M5 10v11h5v-6h4v6h5V10",
        _ => "",
    };
    rsx! {
        svg {
            class: "icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "{path}" }
        }
    }
}

/// Header: brand, connection state, notifications, account.
#[component]
pub fn TopBar(state: Signal<AppState>, sync: Sync) -> Element {
    let mut show_account = use_signal(|| false);
    let mut show_admin = use_signal(|| false);
    let mut show_notifications = use_signal(|| false);
    let mut show_appearance = use_signal(|| false);
    let mut sound_enabled = use_signal(crate::sound::enabled);
    let s = state.read();
    let status = s.status;
    let nickname = s.account.nickname.clone();
    let rating = s.my_rating();
    let online = s.lobby.presence.online_count(now_ms());
    let is_admin = s.is_admin();
    let account_settled = s.account_settled;
    let theme_loaded = s.theme_player == Some(s.account.key.verifying_key().to_bytes());
    // Direct challenges waiting on an answer. The count is of things still to
    // act on, not of things not yet read: a challenge someone accepted or
    // withdrew stops counting on its own, which needs no record of what this
    // device has seen and cannot drift out of step with the network.
    let challenges: Vec<_> = s
        .pending_challenges(now_ms())
        .into_iter()
        .cloned()
        .collect();
    // Visible to admins, and to everyone while nobody has claimed it — hiding
    // an unclaimed race would only mean whoever reads the source wins it.
    let show_admin_entry = is_admin || s.admin_unclaimed();
    drop(s);

    rsx! {
        div { class: "topbar",
            button {
                class: "brand",
                title: "Home",
                onclick: move |_| state.with_mut(|s| s.route = Route::Home),
                span { class: "mark", "aria-hidden": "true", PieceImage { kind: PieceKind::Knight, color: Color::White } }
                span { "FreeChess" }
            }
            div { class: "spacer" }
            div { class: "conn",
                span { class: "dot {status.css()}" }
                span { "{status.label()}" }
            }
            div { class: "conn", "{online} online" }
            span { class: "conn ver", id: "app-version", title: "build {BUILD}", "v{VERSION}" }
            button {
                class: "ghost icon-button", id: "theme-button", title: "Appearance", "aria-label": "Appearance",
                "data-loaded": "{theme_loaded}",
                onclick: move |_| show_appearance.set(true),
                {icon("palette")}
            }
            button {
                class: "ghost icon-button",
                id: "sound-button",
                title: if sound_enabled() { "Mute sounds" } else { "Enable sounds" },
                "aria-label": if sound_enabled() { "Mute sounds" } else { "Enable sounds" },
                "aria-pressed": "{sound_enabled}",
                onclick: move |_| {
                    let enabled = !sound_enabled();
                    crate::sound::set_enabled(enabled);
                    sound_enabled.set(enabled);
                },
                {icon(if sound_enabled() { "sound" } else { "muted" })}
            }
            button {
                class: if challenges.is_empty() { "ghost icon-button" } else { "ghost icon-button has-badge" },
                id: "notifications-button",
                "aria-label": "Notifications",
                title: if challenges.is_empty() {
                    "No challenges waiting for you".to_string()
                } else {
                    format!("{} challenge(s) waiting for you", challenges.len())
                },
                onclick: move |_| show_notifications.set(true),
                {icon("bell")}
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
                    class: "ghost icon-button",
                    id: "admin-button",
                    "aria-label": "Administration",
                    title: if is_admin { "Administration (you are an admin)" } else { "Administration" },
                    onclick: move |_| show_admin.set(true),
                    {icon("settings")}
                }
            }
            button {
                class: "ghost account-button",
                id: "account-button",
                title: "Your account",
                disabled: !account_settled,
                // Whether identity has come back from the delegate yet.
                //
                // Until it has, the app is running on a session key it just
                // generated, and anything signed in that window is signed by
                // somebody who will not exist a moment later. There is no other
                // way to observe that from outside, and guessing at it with
                // timers is what made the browser tests flaky: a claim made too
                // early belonged to a player the next page load had never heard
                // of.
                "data-settled": "{account_settled}",
                onclick: move |_| show_account.set(true),
                {icon("user")}
                span { "{nickname}" }
                span { class: "muted small", "{rating}" }
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
        if show_appearance() {
            AppearanceModal { state, sync, on_close: move |_| show_appearance.set(false) }
        }
    }
}

/// Footer: what this build is, and where to read about the thing it runs on.
///
/// Both links are plain hyperlinks. Nothing is *fetched* from outside — no
/// script, font or image — so the gateway's same-origin CSP is untouched and
/// the app still works with no route to the wider internet. A link the reader
/// may choose to follow is not a dependency.
#[component]
pub fn Footer() -> Element {
    rsx! {
        footer { class: "footer", id: "app-footer",
            div { class: "page",
                div { class: "row small muted",
                    span { id: "footer-version", "FreeChess v{VERSION}" }
                    span { class: "mono", id: "footer-build", "build {BUILD}" }
                    span { class: "spacer" }
                    a {
                        id: "footer-github",
                        href: "https://github.com/mguinhos/freechess",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Source on GitHub"
                    }
                    a {
                        id: "footer-freenet",
                        href: "https://freenet.org/about/faq/",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "What is Freenet?"
                    }
                }
                div { class: "small muted", style: "margin-top:6px",
                    "Online games, ratings and profiles live in Freenet contracts, "
                    "replicated between players. There is no server."
                }
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
    let challenges: Vec<_> = s
        .pending_challenges(now_ms())
        .into_iter()
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
            div { class: "modal", id: "notifications-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Notifications", onclick: move |e| e.stop_propagation(), onmounted: move |_| focus_dialog("notifications-dialog"), onkeydown: move |e| { if e.key() == Key::Escape { on_close.call(()); } },
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
                                    button {
                                        class: "ghost small",
                                        id: "decline-notification-{id}",
                                        onclick: move |_| sync.send(Cmd::DeclineChallenge(id)),
                                        "Decline"
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
            div { class: "modal", id: "account-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Your account", onclick: move |e| e.stop_propagation(), onmounted: move |_| focus_dialog("account-dialog"), onkeydown: move |e| { if e.key() == Key::Escape { on_close.call(()); } },
                h3 {
                    "Your account"
                    button { class: "ghost icon-button", title: "Close account", "aria-label": "Close account", onclick: move |_| on_close.call(()), {icon("close")} }
                }
                div { class: "body",
                    div {
                        label { r#for: "account-nickname", "Nickname" }
                        input {
                            id: "account-nickname",
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
                        label { r#for: "account-import", "Import an account" }
                        input {
                            id: "account-import",
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
                                            sync.send(Cmd::ImportAccount { seed: key.to_bytes() });
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

#[component]
pub fn AppearanceModal(state: Signal<AppState>, sync: Sync, on_close: EventHandler<()>) -> Element {
    let mut theme = use_context::<Signal<Theme>>();
    let current = theme();
    let s = state.read();
    let player = s.account.key.verifying_key().to_bytes();
    let loaded = s.account_settled && s.theme_player == Some(player);
    let saved = loaded
        && s.pending_theme.is_none()
        && s.stored_theme.as_ref().unwrap_or(&Theme::default()) == &current;
    let failed = s.theme_error.is_some() && !saved;
    let save_status = if failed {
        if loaded {
            "Could not save preferences"
        } else {
            "Could not load preferences"
        }
    } else if !loaded {
        "Loading preferences…"
    } else if saved {
        "Saved for this account"
    } else {
        "Saving…"
    };
    drop(s);

    rsx! {
        div { class: "modal-bg", onclick: move |_| on_close.call(()),
            div {
                class: "modal appearance-modal", id: "appearance-dialog", role: "dialog",
                "aria-modal": "true", "aria-label": "Appearance",
                onclick: move |e| e.stop_propagation(),
                onmounted: move |_| focus_dialog("appearance-dialog"),
                onkeydown: move |e| { if e.key() == Key::Escape { on_close.call(()); } },
                h3 { "Appearance" }
                div { class: "body appearance-body",
                    div { class: "appearance-preview",
                        MiniBoard { fen: Board::starting_position().to_fen(), orientation: Color::White }
                        div { class: "theme-preview-detail",
                            div { class: "theme-piece-preview", "aria-label": "Piece preview",
                                PieceImage { kind: PieceKind::Knight, color: Color::White }
                                PieceImage { kind: PieceKind::Knight, color: Color::Black }
                            }
                            p { class: "small muted", "Changes apply immediately." }
                        }
                    }
                    fieldset { class: "appearance-controls", disabled: !loaded, style: "border:0;padding:0;margin:0;min-width:0",
                        div {
                            label { "Board" }
                            div { class: "board-presets",
                                for (name, light, dark) in [
                                    ("Green", "#ebecd0", "#739552"),
                                    ("Brown", "#f0d9b5", "#b58863"),
                                    ("Blue", "#dee3e6", "#8ca2ad"),
                                    ("Gray", "#dedede", "#8a8a8a"),
                                ] {
                                    button {
                                        key: "{name}",
                                        class: if current.light_square == light && current.dark_square == dark { "board-preset active" } else { "board-preset" },
                                        "aria-pressed": "{current.light_square == light && current.dark_square == dark}",
                                        onclick: move |_| theme.with_mut(|t| { t.light_square = light.to_string(); t.dark_square = dark.to_string(); }),
                                        span { class: "board-swatch", style: "background:linear-gradient(135deg, {light} 50%, {dark} 50%)", "aria-hidden": "true" }
                                        "{name}"
                                    }
                                }
                            }
                            div { class: "theme-colors",
                                label { r#for: "light-square-color", "Light squares"
                                    input { id: "light-square-color", r#type: "color", value: "{current.light_square}", oninput: move |e| theme.with_mut(|t| t.light_square = e.value()) }
                                }
                                label { r#for: "dark-square-color", "Dark squares"
                                    input { id: "dark-square-color", r#type: "color", value: "{current.dark_square}", oninput: move |e| theme.with_mut(|t| t.dark_square = e.value()) }
                                }
                            }
                        }
                        div {
                            label { "Pieces" }
                            div { class: "piece-set-options",
                                for (set, name) in [(PieceSet::Neo, "Neo"), (PieceSet::Classic, "Classic")] {
                                    button {
                                        key: "{name}",
                                        class: if current.piece_set == set { "primary" } else { "ghost" },
                                        "aria-pressed": "{current.piece_set == set}",
                                        onclick: move |_| theme.with_mut(|t| t.piece_set = set),
                                        "{name}"
                                    }
                                }
                            }
                            div { class: "theme-colors",
                                label { r#for: "white-piece-color", "White pieces"
                                    input { id: "white-piece-color", r#type: "color", value: "{current.white_piece}", oninput: move |e| theme.with_mut(|t| t.white_piece = e.value()) }
                                }
                                label { r#for: "black-piece-color", "Black pieces"
                                    input { id: "black-piece-color", r#type: "color", value: "{current.black_piece}", oninput: move |e| theme.with_mut(|t| t.black_piece = e.value()) }
                                }
                            }
                        }
                    }
                }
                div { class: "appearance-note small muted", id: "theme-save-status", role: "status", "{save_status}" }
                if failed {
                    button { class: "ghost", onclick: move |_| {
                        if loaded { sync.send(Cmd::SaveTheme { player, theme: theme() }); }
                        else { sync.send(Cmd::LoadTheme); }
                    }, "Retry" }
                }
                div { class: "appearance-note small muted",
                    a { class: "appearance-credits", href: "{PIECE_CREDITS}", target: "_blank", rel: "noopener noreferrer", "Piece artwork credits" }
                }
                div { class: "actions",
                    button { class: "ghost", disabled: !loaded, onclick: move |_| theme.set(Theme::default()), "Reset defaults" }
                    button { class: "primary", onclick: move |_| on_close.call(()), "Close" }
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
            div { class: "modal", id: "new-game-dialog", role: "dialog", "aria-modal": "true", "aria-label": "New game", onclick: move |e| e.stop_propagation(), onmounted: move |_| focus_dialog("new-game-dialog"), onkeydown: move |e| { if e.key() == Key::Escape { on_close.call(()); } },
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
                        div { class: "time-presets",
                            for preset in [(1u32, 0u32), (3, 2), (5, 0), (10, 0), (15, 10), (30, 0)] {
                                button {
                                    key: "{preset.0}-{preset.1}",
                                    class: if minutes() == preset.0 && increment() == preset.1 { "primary" } else { "ghost" },
                                    "aria-pressed": "{minutes() == preset.0 && increment() == preset.1}",
                                    onclick: move |_| { minutes.set(preset.0); increment.set(preset.1); },
                                    "{preset.0}+{preset.1}"
                                }
                            }
                        }
                        div { class: "small muted", style: "margin-top:12px",
                            span { class: "badge {tc.category().to_lowercase()}", "{tc.category()}" }
                            " {minutes} minutes + {increment} seconds per move"
                        }
                    }
                    div {
                        label { "Play as" }
                        div { class: "row",
                            button {
                                class: if color() == Color::White { "primary" } else { "ghost" },
                                "aria-pressed": "{color() == Color::White}",
                                onclick: move |_| color.set(Color::White),
                                "White"
                            }
                            button {
                                class: if color() == Color::Black { "primary" } else { "ghost" },
                                "aria-pressed": "{color() == Color::Black}",
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
    let ready = s.status == ConnStatus::Online && s.account_settled;

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
        .public_open_games(now)
        .into_iter()
        .filter(|e| !s.lobby.administration.is_taken_down(&e.game_id))
        .cloned()
        .collect();
    let invites: Vec<_> = s
        .pending_challenges(now_ms())
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
            div { class: "home-hero",
                div { class: "hero-preview",
                    MiniBoard { fen: Board::starting_position().to_fen(), orientation: Color::White }
                }
                div { class: "hero-content",
                    span { class: "eyebrow", "Your next move" }
                    h1 { "Play chess online" }
                    p { "Choose a time control. Invite a friend or join an open challenge." }
                    div { class: "hero-actions",
                        button {
                            class: "primary hero-play",
                            disabled: !accepting_new_games || !ready,
                            title: if !accepting_new_games { "New games are closed at this address" } else if !ready { "Connecting your account to the node" } else { "Choose your time control and color" },
                            onclick: move |_| show_new.set(true),
                            {icon("play")}
                            "New game"
                        }
                        button {
                            class: "ghost hero-computer",
                            onclick: move |_| state.with_mut(|s| s.route = Route::Practice),
                            "Play computer"
                        }
                    }
                    span { class: "small muted hero-note",
                        if !ready { "Connecting your account..." } else { "Free to play. Your account stays on this device." }
                    }
                }
            }
            div { class: "columns",
                div {
                    div { class: "row search-row", style: "margin-bottom:16px",
                        input {
                            placeholder: "Search games by player or id\u{2026}",
                            "aria-label": "Search games",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
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
                                            disabled: !ready,
                                            onclick: {
                                                let id = entry.game_id;
                                                move |_| sync.send(Cmd::JoinGame(id))
                                            },
                                            "Accept"
                                        }
                                        // Declining is published, not hidden
                                        // locally: the point is that the
                                        // challenger finds out, instead of
                                        // waiting on an answer that is never
                                        // coming.
                                        button {
                                            class: "ghost",
                                            id: "decline-{entry.game_id}",
                                            onclick: {
                                                let id = entry.game_id;
                                                move |_| sync.send(Cmd::DeclineChallenge(id))
                                            },
                                            "Decline"
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
                                                disabled: !ready,
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
                                                        role: "button",
                                                        tabindex: "0",
                                                        "aria-label": "Watch {white} versus {black}",
                                                        onkeydown: move |e| {
                                                            if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                                                                sync.send(Cmd::WatchGame(id));
                                                                state.with_mut(|s| s.route = Route::Game(id));
                                                            }
                                                        },
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


                }

                div {
                    // Identified for the same reason as the players panel:
                    // a nickname appears in several lists, so a test that
                    // matches on one alone reads an arbitrary row.
                    div { class: "panel", id: "ranking-panel",
                        h2 { "Ranking" }
                        if ranked.is_empty() {
                            div { class: "empty", "No rated games yet." }
                        } else {
                            div { class: "list",
                                for (i, entry) in ranked.iter().take(20).enumerate() {
                                    div { key: "{entry.player_id()}", id: "rank-{entry.player_id()}", class: "list-item",
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
                                            disabled: !ready || !accepting_new_games,
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

fn replay_position(ply: usize, total_plies: usize) -> Option<usize> {
    // Returning to the current position resumes live updates.
    (ply < total_plies).then_some(ply)
}

/// A single game — playing or spectating, plus the replay scrubber once it is
/// over.
#[component]
pub fn GameView(state: Signal<AppState>, sync: Sync, game_id: GameId) -> Element {
    let mut viewing_ply = use_signal(|| None::<usize>);
    let mut challenge_target = use_signal(|| None::<(String, ed25519_dalek::VerifyingKey)>);
    let mut flipped = use_signal(|| false);
    let mut result_dismissed = use_signal(|| false);
    let mut confirm_resign = use_signal(|| false);
    let mut show_pgn = use_signal(|| false);
    let mut copy_feedback = use_signal(String::new);

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
    let ready = s.status == ConnStatus::Online && s.account_settled;
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
    let my_key = state.read().account.key.verifying_key();
    let my_color = game.color_of(&my_key).or_else(|| {
        game.setup
            .get()
            .filter(|setup| setup.creator == my_key)
            .map(|setup| setup.setup.creator_plays)
    });
    let orientation = if flipped() {
        my_color.unwrap_or(Color::White).opposite()
    } else {
        my_color.unwrap_or(Color::White)
    };
    let result = game.result();
    let is_over = result.is_over();

    // A past position is not playable.
    let total_plies = replay.ply();
    let shown_ply = viewing_ply().unwrap_or(total_plies).min(total_plies);
    let scrubbing = shown_ply != total_plies;
    let fen = replay.fen_at(shown_ply).to_string();

    let playable = if scrubbing || is_over || !ready {
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

    let (captured_by_white, captured_by_black) = captured_material(&replay.outcomes()[..shown_ply]);
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

    // Whether the opponent's clock has emptied. There is no button for this any
    // more: winning on time is not something one player does to another, it is
    // what happens when a clock reaches zero, and the client files it by itself.
    // Offering it as a control invited pressing it as soon as the contract would
    // accept — which is when the opponent went quiet, not when their time ran
    // out. This line only tells the player what is about to happen.
    let opponent_flagged = my_color
        .filter(|_| !is_over)
        .map(|mine| game.time_remaining(mine.opposite(), now) == 0)
        .unwrap_or(false);

    let sans = replay.san_list();
    let link = share_link(game_id);
    let pgn = replay.to_pgn(
        &[
            ("Event", "FreeChess online game".to_string()),
            ("Site", link.clone()),
            ("White", white_name.clone()),
            ("Black", black_name.clone()),
        ],
        result.pgn_token(),
    );
    let pgn_copy = pgn.clone();
    let pgn_download = format!(
        "data:application/x-chess-pgn;charset=utf-8,{}",
        js_sys::encode_uri_component(&pgn)
    );
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

    let seat = |name: String, ms: i64, color: Color, captured: Vec<PieceKind>| {
        let active = !is_over && to_move == color && game.opponent.get().is_some();
        let low = ms < 20_000;
        let mut class = String::from("clock");
        if active {
            class.push_str(" active");
        }
        if active && low {
            class.push_str(" low");
        }
        rsx! {
            div { class: "seat", "data-color": if color == Color::White { "white" } else { "black" },
                div { class: "seat-avatar", PieceImage { kind: PieceKind::Pawn, color } }
                div { class: "seat-details",
                    span { class: "name", "{name}" }
                    span { class: "captured",
                        for kind in captured {
                            PieceImage { kind, color: color.opposite() }
                        }
                    }
                }
                span { class: "spacer" }
                span { class: "{class}", "aria-label": "{name}: {format_clock(ms)} remaining", "{format_clock(ms)}" }
            }
        }
    };

    rsx! {
        div { class: "page",
            div { class: "game-toolbar",
                button { class: "ghost small", onclick: move |_| state.with_mut(|s| s.route = Route::Home), {icon("home")} "Lobby" }
                span { class: "spacer" }
                button {
                    class: "ghost small", id: "flip-board", title: "Flip board",
                    onclick: move |_| flipped.set(!flipped()),
                    {icon("flip")} "Flip board"
                }
            }
            div { class: "game-layout",
                div {
                    if orientation == Color::White {
                        {seat(black_name.clone(), black_ms, Color::Black, captured_by_black.clone())}
                    } else {
                        {seat(white_name.clone(), white_ms, Color::White, captured_by_white.clone())}
                    }
                    div { class: "board-wrap",
                        GameBoard {
                            fen: fen.clone(),
                            orientation,
                            playable,
                            last_move,
                            on_move: move |mv: Move| {
                                sync.send(Cmd::PlayMove { game: game_id, mv });
                            },
                        }
                        if is_over && !scrubbing && !result_dismissed() {
                            div { class: "result-overlay",
                                div { class: "result-card",
                                    div { class: "headline", "{result_headline(result, my_color)}" }
                                    div { class: "muted small", "{white_name} vs {black_name}" }
                                    button { class: "primary", onclick: move |_| result_dismissed.set(true), "Review game" }
                                }
                            }
                        }
                    }
                    if orientation == Color::White {
                        {seat(white_name.clone(), white_ms, Color::White, captured_by_white)}
                    } else {
                        {seat(black_name.clone(), black_ms, Color::Black, captured_by_black)}
                    }
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
                                        button {
                                            class: if shown_ply == i * 2 + 1 { "mv current" } else { "mv" },
                                            onclick: move |_| viewing_ply.set(replay_position(i * 2 + 1, total_plies)),
                                            "aria-label": "Move {i + 1}, White: {pair[0]}",
                                            "{pair[0]}"
                                        }
                                        if pair.len() > 1 {
                                            button {
                                                class: if shown_ply == i * 2 + 2 { "mv current" } else { "mv" },
                                                onclick: move |_| viewing_ply.set(replay_position(i * 2 + 2, total_plies)),
                                                "aria-label": "Move {i + 1}, Black: {pair[1]}",
                                                "{pair[1]}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "body",
                            div { class: "replay-bar",
                                button { class: "ghost", title: "Starting position", "aria-label": "Starting position", disabled: shown_ply == 0, onclick: move |_| viewing_ply.set(replay_position(0, total_plies)), {icon("first")} }
                                button {
                                    class: "ghost",
                                    title: "Previous move", "aria-label": "Previous move", disabled: shown_ply == 0,
                                    onclick: move |_| viewing_ply.set(replay_position(shown_ply.saturating_sub(1), total_plies)),
                                    {icon("previous")}
                                }
                                button {
                                    class: "ghost",
                                    title: "Next move", "aria-label": "Next move", disabled: shown_ply == total_plies,
                                    onclick: move |_| viewing_ply.set(replay_position(shown_ply + 1, total_plies)),
                                    {icon("next")}
                                }
                                button { class: "ghost", title: "Latest position", "aria-label": "Latest position", disabled: !scrubbing, onclick: move |_| viewing_ply.set(None), {icon("last")} }
                            }
                            if scrubbing {
                                div {
                                    class: "banner warn small",
                                    id: "replay-position",
                                    style: "margin-top:10px",
                                    "Viewing position {shown_ply} of {total_plies}. "
                                    button {
                                        class: "text-button",
                                        onclick: move |_| viewing_ply.set(None),
                                        if is_over { "Latest position" } else { "Jump to live" }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "panel",
                        h2 { "Game" }
                        div { class: "body",
                            // A pending offer is neither playing nor
                            // spectating, and saying both at once — which this
                            // did — is worse than saying neither. Until the
                            // creator countersigns there is no colour to have,
                            // so the honest line is that we are waiting.
                            div { class: "small muted", style: "margin-bottom:10px",
                                if my_color.is_some() { "You are playing this game." }
                                else if offer_pending { "You have asked for this seat." }
                                else { "You are spectating." }
                            }
                            div { class: "game-state", role: "status",
                                if is_over { "{result_headline(result, my_color)}" }
                                else if game.opponent.get().is_none() { "Waiting for an opponent" }
                                else if !ready { "Reconnecting..." }
                                else if scrubbing { "Reviewing the game" }
                                else if playable.is_some() { "Your turn" }
                                else if my_color.is_some() { "Opponent's turn" }
                                else if to_move == Color::White { "White to move" }
                                else { "Black to move" }
                            }
                            if my_color.is_some() && game.opponent.get().is_none() {
                                div { class: "small muted", "Share the link below to invite a friend." }
                            }
                            if can_join {
                                button {
                                    class: "primary",
                                    style: "width:100%",
                                    disabled: !ready,
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
                                    disabled: !ready,
                                    onclick: move |_| confirm_resign.set(true),
                                    "Resign"
                                }
                            }
                            if opponent_flagged {
                                div { class: "small muted", id: "flag-fall-notice", style: "margin-top:8px",
                                    "Your opponent's clock has run out. Ending the game\u{2026}"
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
                                    "{link}"
                                }
                            }
                            div { class: "share-actions",
                                button {
                                    class: "ghost small", id: "copy-game-link",
                                    onclick: move |_| {
                                        let text = share_link(game_id);
                                        spawn(async move {
                                            copy_feedback.set(if copy_to_clipboard(text).await {
                                                "Game link copied.".to_string()
                                            } else {
                                                "Select the link above to copy it.".to_string()
                                            });
                                        });
                                    },
                                    {icon("copy")} "Copy link"
                                }
                                button { class: "ghost small", id: "export-pgn", onclick: move |_| { copy_feedback.set(String::new()); show_pgn.set(true); }, {icon("download")} "PGN" }
                            }
                            if !copy_feedback().is_empty() {
                                div { class: "small muted", role: "status", "{copy_feedback}" }
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
                                            disabled: !ready,
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

        if confirm_resign() && !is_over {
            div { class: "modal-bg", onclick: move |_| confirm_resign.set(false),
                div {
                    class: "modal", id: "resign-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Resign game",
                    onmounted: move |_| focus_dialog("resign-dialog"),
                    onclick: move |e| e.stop_propagation(),
                    onkeydown: move |e| { if e.key() == Key::Escape { confirm_resign.set(false); } },
                    h3 { "Resign this game?" }
                    div { class: "body", "Your opponent will win. This cannot be undone." }
                    div { class: "actions",
                        button { class: "ghost", onclick: move |_| confirm_resign.set(false), "Keep playing" }
                        button {
                            class: "danger", id: "confirm-resign", disabled: !ready,
                            onclick: move |_| { sync.send(Cmd::Resign(game_id)); confirm_resign.set(false); },
                            "Resign game"
                        }
                    }
                }
            }
        }
        if show_pgn() {
            div { class: "modal-bg", onclick: move |_| show_pgn.set(false),
                div { class: "modal pgn-modal", id: "pgn-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Export PGN", onclick: move |e| e.stop_propagation(), onmounted: move |_| focus_dialog("pgn-dialog"), onkeydown: move |e| { if e.key() == Key::Escape { show_pgn.set(false); } },
                    h3 { "Export PGN" }
                    div { class: "body",
                        textarea { class: "pgn-text mono", id: "pgn-text", "aria-label": "Game PGN", readonly: true, value: "{pgn}" }
                        if !copy_feedback().is_empty() {
                            div { class: "small muted", role: "status", "{copy_feedback}" }
                        }
                        div { class: "actions",
                            button { class: "ghost", onclick: move |_| show_pgn.set(false), "Close" }
                            a { class: "button ghost", href: "{pgn_download}", download: "freechess-{game_id}.pgn", {icon("download")} "Download" }
                            button {
                                class: "primary", id: "copy-pgn",
                                onclick: move |_| {
                                    let text = pgn_copy.clone();
                                    spawn(async move {
                                        copy_feedback.set(if copy_to_clipboard(text).await {
                                            "PGN copied.".to_string()
                                        } else {
                                            "Select the PGN to copy it.".to_string()
                                        });
                                    });
                                },
                                {icon("copy")} "Copy PGN"
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

async fn copy_to_clipboard(text: String) -> bool {
    let mut eval = document::eval(
        r#"
        const text = await dioxus.recv();
        let copied = false;
        try {
            await navigator.clipboard.writeText(text);
            copied = true;
        } catch (_) {
            // The gateway's sandbox may deny the Clipboard API.
            const previous = document.activeElement;
            const input = document.createElement('textarea');
            input.value = text;
            input.style.cssText = 'position:fixed;left:-9999px;top:0';
            document.body.appendChild(input);
            input.select();
            try { copied = document.execCommand('copy'); } catch (_) {}
            input.remove();
            previous?.focus();
        }
        dioxus.send(copied);
    "#,
    );
    if eval.send(text).is_err() {
        return false;
    }
    eval.recv::<bool>().await.unwrap_or(false)
}

fn focus_dialog(id: &str) {
    let eval = document::eval(
        r#"
        const dialog = document.getElementById(await dioxus.recv());
        if (!dialog) return;
        const previous = document.activeElement;
        const controls = () => [...dialog.querySelectorAll('button, a[href], input, select, textarea, [tabindex]')]
            .filter(el => !el.disabled && el.tabIndex >= 0 && el.getClientRects().length);
        dialog.tabIndex = -1;
        (controls()[0] || dialog).focus();
        dialog.addEventListener('keydown', event => {
            if (event.key !== 'Tab') return;
            const items = controls();
            const first = items[0], last = items[items.length - 1];
            if (!first) { event.preventDefault(); dialog.focus(); }
            else if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog)) {
                event.preventDefault(); last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault(); first.focus();
            }
        });
        const observer = new MutationObserver(() => {
            if (!dialog.isConnected) {
                observer.disconnect();
                if (previous?.isConnected) previous.focus();
            }
        });
        observer.observe(document.body, { childList: true, subtree: true });
    "#,
    );
    let _ = eval.send(id);
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
    let mut dismissed = use_signal(|| false);
    // Only prompt once the delegate has answered; before that we do not yet
    // know whether this player already has a name.
    let mut waited = use_signal(|| false);

    let s = state.read();
    let needs_name = s.nickname_unset;
    let settled = s.account_settled;
    let player_id = s.me();
    let practicing = matches!(s.route, Route::Practice);
    drop(s);

    use_future(move || async move {
        crate::gloo_sleep(4000).await;
        waited.set(true);
    });

    if dismissed() || practicing || !needs_name || !settled || !waited() {
        return rsx! {};
    }

    if web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector("[role=dialog]:not(#nickname-dialog)")
                .ok()
                .flatten()
        })
        .is_some()
    {
        return rsx! {};
    }

    rsx! {
        div { class: "modal-bg", id: "nickname-prompt",
            div {
                class: "modal", id: "nickname-dialog", style: "max-width:440px",
                role: "dialog", "aria-modal": "true", "aria-label": "Choose a nickname",
                onmounted: move |_| focus_dialog("nickname-dialog"),
                onkeydown: move |e| { if e.key() == Key::Escape { dismissed.set(true); } },
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
                        button { class: "ghost", id: "nickname-later", onclick: move |_| dismissed.set(true), "Later" }
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
                    button {
                        class: "text-button small", id: "nickname-play-computer",
                        onclick: move |_| { dismissed.set(true); state.with_mut(|s| s.route = Route::Practice); },
                        "Play computer instead"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::replay_position;
    use chess_core::chess::{Game, Move};

    #[test]
    fn returning_to_current_position_follows_the_next_move() {
        let mut game = Game::new();
        game.play(Move::from_uci("e2e4").unwrap()).unwrap();
        let start = replay_position(0, game.ply());
        assert_eq!(start, Some(0));
        let next = replay_position(start.unwrap() + 1, game.ply());

        game.play(Move::from_uci("e7e5").unwrap()).unwrap();
        assert_eq!(
            game.fen_at(next.unwrap_or(game.ply())),
            game.current_fen(),
            "Next back to the current position must resume following live moves"
        );
        assert_eq!(next, None);
    }

    #[test]
    fn only_past_positions_pin_the_replay() {
        assert_eq!(replay_position(0, 0), None);
        assert_eq!(replay_position(0, 4), Some(0));
        assert_eq!(replay_position(3, 4), Some(3));
        assert_eq!(replay_position(4, 4), None);
        assert_eq!(replay_position(5, 4), None);
    }
}
