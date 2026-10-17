//! Stockfish runs in a sandbox-compatible worker; the shared rules validate its moves.

use chess_core::chess::{Game, Move};
use dioxus::prelude::*;
use serde::Deserialize;

// Build scripts restore this folder verbatim after dx processes its assets.
pub const STOCKFISH: Asset = asset!("/assets/stockfish", AssetOptions::folder());

pub fn start(game: &Game, skill_level: u8) -> Result<document::Eval, String> {
    let moves = game
        .moves()
        .iter()
        .map(|mv| mv.to_uci())
        .collect::<Vec<_>>()
        .join(" ");
    let position = format!("position fen {} moves {moves}", game.fen_at(0));
    let eval = document::eval(include_str!("engine.js"));
    eval.send((
        format!("{STOCKFISH}/stockfish-19-lite-single.js"),
        format!("{STOCKFISH}/stockfish-19-lite-single.wasm"),
        position,
        skill_level,
    ))
    .map_err(|_| "Could not start Stockfish".to_string())?;
    Ok(eval)
}

#[derive(Deserialize)]
struct Reply {
    best_move: Option<String>,
    error: Option<String>,
}

pub async fn finish(mut eval: document::Eval, game: &Game) -> Result<Move, String> {
    let reply: Reply = eval
        .recv()
        .await
        .map_err(|_| "Stockfish stopped unexpectedly".to_string())?;
    if let Some(error) = reply.error {
        return Err(error);
    }
    validate_move(game, reply.best_move.as_deref().unwrap_or_default())
}

fn validate_move(game: &Game, uci: &str) -> Result<Move, String> {
    uci.is_ascii()
        .then_some(uci)
        .and_then(Move::from_uci)
        .filter(|mv| game.is_legal(*mv))
        .ok_or_else(|| "Stockfish returned an invalid move".to_string())
}

pub fn cancel(mut active: Signal<Option<document::Eval>>) {
    if let Some(eval) = active.write().take() {
        let _ = eval.send("cancel");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_replies_must_be_legal_in_the_requested_position() {
        let mut game = Game::new();
        game.play(Move::from_uci("e2e4").unwrap()).unwrap();
        assert_eq!(validate_move(&game, "e7e5").unwrap().to_uci(), "e7e5");
        for invalid in ["e2e4", "e7e4", "(none)", "0000", "not a move", "∑x"] {
            assert!(validate_move(&game, invalid).is_err());
        }
    }
}
