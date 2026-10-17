//! Untimed local practice against the bundled Stockfish engine.

use crate::board::{GameBoard, PieceImage};
use crate::{engine, sound};
use chess_core::chess::{Color, Game, GameStatus, Move, PieceKind};
use dioxus::prelude::*;

fn play_move(mut game: Signal<Game>, mv: Move) {
    let clip = game.with_mut(|game| {
        let outcome = game.play(mv).ok()?.clone();
        Some(sound::for_move(&outcome, game.status().is_over()))
    });
    if let Some(clip) = clip {
        sound::play(clip);
    }
}

fn undo_turn(game: &Game) -> Game {
    let keep = game.ply().saturating_sub(1) / 2 * 2;
    Game::from_moves(&game.moves()[..keep]).expect("previous moves are legal")
}

#[component]
pub fn PracticeView() -> Element {
    let mut game = use_signal(Game::new);
    let mut difficulty = use_signal(|| 5u8);
    let mut revision = use_signal(|| 0u64);
    let mut failure = use_signal(|| None::<String>);
    let mut active = use_signal(|| None::<document::Eval>);
    use_drop(move || engine::cancel(active));
    use_future(move || async move {
        loop {
            crate::gloo_sleep(60).await;
            if game.peek().side_to_move() != Color::Black
                || game.peek().status().is_over()
                || failure.peek().is_some()
            {
                continue;
            }
            let position = game.peek().clone();
            let requested = *revision.peek();
            let skill_level = *difficulty.peek();
            let result = match engine::start(&position, skill_level) {
                Ok(eval) => {
                    active.set(Some(eval));
                    let result = engine::finish(eval, &position).await;
                    active.set(None);
                    result
                }
                Err(error) => Err(error),
            };
            if requested != *revision.peek() || game.peek().current_fen() != position.current_fen()
            {
                continue;
            }
            match result {
                Ok(mv) => play_move(game, mv),
                Err(error) => failure.set(Some(error)),
            }
        }
    });
    let position = game.read();
    let status = position.status();
    let thinking = !status.is_over() && position.side_to_move() == Color::Black;
    let playable =
        (!status.is_over() && !thinking && failure.read().is_none()).then_some(Color::White);
    let fen = position.current_fen().to_string();
    let last_move = position.moves().last().copied();
    let sans = position.san_list();
    let can_undo = position.ply() > 0;
    let label = match status {
        GameStatus::InProgress if failure.read().is_some() => "Stockfish unavailable",
        GameStatus::InProgress if thinking => "Stockfish is thinking…",
        GameStatus::InProgress if position.board().is_check() => "Your turn — in check",
        GameStatus::InProgress => "Your turn",
        GameStatus::Checkmate {
            winner: Color::White,
        } => "You won by checkmate",
        GameStatus::Checkmate {
            winner: Color::Black,
        } => "Stockfish won by checkmate",
        GameStatus::Stalemate => "Draw by stalemate",
        GameStatus::InsufficientMaterial => "Draw by insufficient material",
        GameStatus::FiftyMoveRule => "Draw by fifty-move rule",
        GameStatus::ThreefoldRepetition => "Draw by repetition",
    };
    let engine_status = if failure.read().is_some() {
        "error"
    } else if status.is_over() {
        "over"
    } else if thinking {
        "thinking"
    } else {
        "ready"
    };
    drop(position);
    rsx! {
        div { class: "page",
            div { class: "game-toolbar",
                h1 { "Play computer" }
                span { class: "spacer" }
                span { class: "tag", "Untimed" }
            }
            div { class: "game-layout",
                div {
                    div { class: "seat",
                        div { class: "seat-avatar", PieceImage { kind: PieceKind::King, color: Color::Black } }
                        span { class: "name", "Stockfish 19 Lite" }
                        span { class: "spacer" }
                        a { class: "muted small", id: "stockfish-credits", href: "{engine::STOCKFISH}/credits.html", target: "_blank", rel: "noopener noreferrer", "Open source" }
                    }
                    div { class: "board-wrap",
                        GameBoard { fen, orientation: Color::White, playable, last_move,
                            on_move: move |mv| {
                                if game.peek().side_to_move() == Color::White {
                                    play_move(game, mv);
                                }
                            }
                        }
                    }
                    div { class: "seat",
                        div { class: "seat-avatar", PieceImage { kind: PieceKind::King, color: Color::White } }
                        span { class: "name", "You" }
                        span { class: "spacer" }
                        span { class: "muted small", "White" }
                    }
                }
                div { class: "panel",
                    h2 { id: "engine-status", "data-status": engine_status, role: "status", aria_live: "polite", "{label}" }
                    if let Some(error) = failure() {
                        p { id: "engine-error", role: "alert", class: "small", "{error}" }
                        button { id: "retry-stockfish", onclick: move |_| failure.set(None), "Retry Stockfish" }
                    }
                    div { class: "body",
                        label { r#for: "engine-difficulty", "Difficulty" }
                        select {
                            id: "engine-difficulty",
                            value: "{difficulty}",
                            onchange: move |event| {
                                if let Ok(level) = event.value().parse::<u8>() {
                                    difficulty.set(level.min(20));
                                    revision += 1;
                                    engine::cancel(active);
                                    failure.set(None);
                                }
                            },
                            option { value: "0", "Easy" }
                            option { value: "5", "Medium" }
                            option { value: "10", "Hard" }
                            option { value: "20", "Maximum" }
                        }
                    }
                    p { class: "muted small", "Select a piece, then its destination. Dragging and keyboard controls work too." }
                    div { class: "replay-bar",
                        button { class: "primary", onclick: move |_| {
                            revision += 1;
                            engine::cancel(active);
                            failure.set(None);
                            game.set(Game::new());
                        }, "New game" }
                        button { disabled: !can_undo, onclick: move |_| {
                            revision += 1;
                            engine::cancel(active);
                            failure.set(None);
                            let restored = undo_turn(&game.peek());
                            game.set(restored);
                        }, "Undo turn" }
                    }
                    h2 { "Moves" }
                    div { class: "moves",
                        if sans.is_empty() { div { class: "empty", "Play your first move." } }
                        for (i, pair) in sans.chunks(2).enumerate() {
                            div { class: "mv-row",
                                span { class: "num", "{i + 1}." }
                                span { class: "mv", "{pair[0]}" }
                                if let Some(reply) = pair.get(1) {
                                    span { class: "mv", "{reply}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undo_returns_to_humans_previous_turn_even_while_computer_waits() {
        let mut game = Game::new();
        game.play(Move::from_uci("e2e4").unwrap()).unwrap();
        assert_eq!(undo_turn(&game).ply(), 0);
        game.play(Move::from_uci("e7e5").unwrap()).unwrap();
        assert_eq!(undo_turn(&game).ply(), 0);
        game.play(Move::from_uci("g1f3").unwrap()).unwrap();
        let restored = undo_turn(&game);
        assert_eq!(restored.ply(), 2);
        assert_eq!(restored.side_to_move(), Color::White);
    }
}
