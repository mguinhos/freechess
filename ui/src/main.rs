//! FreeChess — a decentralized chess site.
//!
//! Everything the app shows lives in Freenet contracts, replicated to every
//! peer: the games, the lobby, the ranking, presence, and the history. The one
//! piece of private data is the player's signing key, which stays on the device
//! in the delegate.
//!
//! No external requests are made at runtime. The gateway serves the app under a
//! same-origin CSP, so there are no CDN scripts, no web fonts and no remote
//! images: the pieces are Unicode glyphs and the stylesheet ships in the
//! bundle.

mod app;
mod board;
mod freenet;
mod identity;
mod views;

use app::{AppState, Route};
use chess_core::identity::GameId;
use dioxus::prelude::*;

/// The stylesheet, bundled by the asset pipeline. Going through `asset!` rather
/// than a plain path means the emitted URL is rewritten with the gateway's
/// base path, so the CSS resolves under `/v1/contract/web/<key>/` too.
const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let account = use_signal(identity::load_or_create);
    let mut state = use_signal(|| AppState::new(account()));
    let sync = app::use_sync(state);

    // A game id in the query string opens straight into that game — this is
    // what makes a replay link shareable.
    use_effect(move || {
        if let Some(id) = game_id_from_url() {
            state.with_mut(|s| s.route = Route::Game(id));
            sync.send(app::Cmd::WatchGame(id));
        }
    });

    // Watching a game needs its creator's key, which comes from the lobby. On a
    // cold load the lobby has not arrived yet, so that first attempt finds
    // nothing — retry once the lobby lands. Without this, opening a shared
    // replay link in a fresh browser shows an empty board forever.
    let mut requested = use_signal(|| None::<GameId>);
    use_effect(move || {
        let s = state.read();
        let (Route::Game(id) | Route::Replay(id)) = s.route.clone() else {
            return;
        };
        let missing = !s.games.contains_key(&id);
        let known_to_lobby = s.lobby.games.get(&id).is_some();
        drop(s);

        if missing && known_to_lobby && requested() != Some(id) {
            requested.set(Some(id));
            sync.send(app::Cmd::WatchGame(id));
        }
    });

    // Settle the account quickly at startup. The heartbeat also retries, but at
    // 30s intervals — too slow to keep the first page load waiting on.
    use_future(move || async move {
        for _ in 0..15 {
            if state.read().account_settled {
                return;
            }
            sync.send(app::Cmd::LoadAccount);
            gloo_sleep(2000).await;
        }
    });

    // Heartbeat. Announcing the game being watched is what lets a spectator
    // list appear, and therefore what makes "invite that spectator" possible.
    use_future(move || async move {
        loop {
            let watching = match state.read().route.clone() {
                Route::Game(id) | Route::Replay(id) => Some(id),
                _ => None,
            };
            sync.send(app::Cmd::Heartbeat { watching });

            // Keep asking the delegate for the account until it answers. The
            // registration and the first request are separate messages, so a
            // request sent before the delegate is registered is silently
            // dropped — one attempt at startup is not enough.
            if !state.read().account_settled {
                sync.send(app::Cmd::LoadAccount);
            }

            gloo_sleep(chess_core::presence::HEARTBEAT_INTERVAL_MS as u32).await;
        }
    });

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        views::TopBar { state, sync }
        views::StatusBanner { state }
        views::NoticeBanners { state }
        match state.read().route.clone() {
            Route::Home => rsx! { views::HomeView { state, sync } },
            Route::Game(id) => rsx! {
                views::GameView { state, sync, game_id: id, replay_mode: false }
            },
            Route::Replay(id) => rsx! {
                views::GameView { state, sync, game_id: id, replay_mode: true }
            },
            Route::Profile(_) => rsx! { views::HomeView { state, sync } },
        }
    }
}

/// `?game=<base58>` from the current URL.
fn game_id_from_url() -> Option<GameId> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    GameId::from_base58(&params.get("game")?)
}

/// A timer future, without pulling in another dependency.
async fn gloo_sleep(millis: u32) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let closure = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = tx.send(());
    });
    if let Some(window) = web_sys::window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.unchecked_ref(),
            millis as i32,
        );
    }
    let _ = rx.await;
}

use wasm_bindgen::JsCast;
