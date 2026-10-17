//! The chess delegate: durable, device-local storage for the player's account.
//!
//! Everything else in FreeChess is public replicated state. This is the
//! exception — it runs inside the user's own Freenet core, its secret store
//! never leaves the device, and it is what lets a player close the browser and
//! come back as the same person.
//!
//! See [`chess_delegate_protocol`] for the protocol and for why the seed is
//! handed back to the UI rather than the delegate signing on its behalf.

#![allow(unexpected_cfgs)]

use chess_delegate_protocol::{
    ChessDelegateRequest, ChessDelegateResponse, Theme, ACCOUNT_SEED_KEY, NICKNAME_KEY,
    THEME_KEY_PREFIX,
};
use freenet_stdlib::prelude::{
    delegate, ApplicationMessage, DelegateCtx, DelegateError, DelegateInterface,
    InboundDelegateMsg, MessageOrigin, OutboundDelegateMsg, Parameters,
};

pub struct ChessDelegate;

#[delegate]
impl DelegateInterface for ChessDelegate {
    fn process(
        ctx: &mut DelegateCtx,
        _parameters: Parameters<'static>,
        _origin: Option<MessageOrigin>,
        message: InboundDelegateMsg,
    ) -> Result<Vec<OutboundDelegateMsg>, DelegateError> {
        // Only application messages are meaningful here: this delegate holds a
        // secret and answers questions about it. `InboundDelegateMsg` is
        // `#[non_exhaustive]`, so unknown variants fall through to the same
        // no-op rather than panicking — a panic inside delegate WASM would take
        // down the runtime for the whole app.
        let app_msg = match message {
            InboundDelegateMsg::ApplicationMessage(msg) => msg,
            _ => return Ok(vec![]),
        };

        // An already-processed message is an echo of our own reply.
        if app_msg.processed {
            return Ok(vec![]);
        }

        let context = app_msg.context.clone();
        let response = match ChessDelegateRequest::from_bytes(&app_msg.payload) {
            Ok(request) => handle(ctx, request),
            Err(e) => ChessDelegateResponse::Error {
                message: format!("could not decode request: {e}"),
            },
        };

        let payload = match response.to_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                // Encoding our own response should not fail; if it somehow
                // does, report it in the same shape rather than dropping the
                // reply and leaving the UI waiting forever.
                ChessDelegateResponse::Error {
                    message: format!("could not encode response: {e}"),
                }
                .to_bytes()
                .unwrap_or_default()
            }
        };

        Ok(vec![OutboundDelegateMsg::ApplicationMessage(
            ApplicationMessage::new(payload)
                .with_context(context)
                .processed(true),
        )])
    }
}

fn handle(ctx: &mut DelegateCtx, request: ChessDelegateRequest) -> ChessDelegateResponse {
    match request {
        ChessDelegateRequest::GetAccount => match ctx.get_secret(ACCOUNT_SEED_KEY) {
            Some(bytes) => match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(seed) => ChessDelegateResponse::Account { seed: Some(seed) },
                // Stored bytes of the wrong length mean a corrupted or
                // foreign-format secret. Report it rather than silently
                // handing back a different account.
                Err(_) => ChessDelegateResponse::Error {
                    message: format!("stored account seed is {} bytes, expected 32", bytes.len()),
                },
            },
            None => ChessDelegateResponse::Account { seed: None },
        },

        ChessDelegateRequest::StoreAccount { seed } => {
            if ctx.set_secret(ACCOUNT_SEED_KEY, &seed) {
                ChessDelegateResponse::AccountStored { seed }
            } else {
                ChessDelegateResponse::Error {
                    message: "the secret store rejected the account seed".to_string(),
                }
            }
        }

        ChessDelegateRequest::ClearAccount => {
            ctx.remove_secret(ACCOUNT_SEED_KEY);
            ctx.remove_secret(NICKNAME_KEY);
            ChessDelegateResponse::Stored
        }

        ChessDelegateRequest::SetNickname { nickname } => {
            if ctx.set_secret(NICKNAME_KEY, nickname.as_bytes()) {
                ChessDelegateResponse::Stored
            } else {
                ChessDelegateResponse::Error {
                    message: "the secret store rejected the nickname".to_string(),
                }
            }
        }

        ChessDelegateRequest::GetNickname => {
            let nickname = ctx
                .get_secret(NICKNAME_KEY)
                .and_then(|bytes| String::from_utf8(bytes).ok());
            ChessDelegateResponse::Nickname { nickname }
        }

        ChessDelegateRequest::GetTheme { player } => {
            let key = theme_key(&player);
            let theme = match ctx.get_secret(&key) {
                Some(bytes) => match decode_theme(&bytes) {
                    Ok(theme) => Some(theme),
                    Err(message) => return ChessDelegateResponse::Error { message },
                },
                None if ctx.has_secret(&key) => {
                    return ChessDelegateResponse::Error {
                        message: "could not read the stored theme".to_string(),
                    };
                }
                None => None,
            };
            ChessDelegateResponse::Theme { player, theme }
        }

        ChessDelegateRequest::SetTheme { player, theme } => {
            let bytes = match encode_theme(&theme) {
                Ok(bytes) => bytes,
                Err(message) => return ChessDelegateResponse::Error { message },
            };
            if ctx.set_secret(&theme_key(&player), &bytes) {
                ChessDelegateResponse::ThemeSaved { player, theme }
            } else {
                ChessDelegateResponse::Error {
                    message: "the secret store rejected the theme".to_string(),
                }
            }
        }
    }
}

fn theme_key(player: &[u8; 32]) -> Vec<u8> {
    [THEME_KEY_PREFIX, player.as_slice()].concat()
}

fn encode_theme(theme: &Theme) -> Result<Vec<u8>, String> {
    if !theme.valid() {
        return Err("theme colors must be #RRGGBB hex values".to_string());
    }
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(theme, &mut bytes)
        .map_err(|error| format!("could not encode theme: {error}"))?;
    Ok(bytes)
}

fn decode_theme(bytes: &[u8]) -> Result<Theme, String> {
    let mut remaining = bytes;
    let theme: Theme = ciborium::de::from_reader(&mut remaining)
        .map_err(|error| format!("could not decode stored theme: {error}"))?;
    if !remaining.is_empty() || !theme.valid() {
        return Err("the stored theme is invalid".to_string());
    }
    Ok(theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_delegate_protocol::PieceSet;

    #[test]
    fn themes_have_independent_keys_for_full_player_identities() {
        let first = [1; 32];
        let mut second = first;
        second[31] = 2;
        let first_key = theme_key(&first);
        assert_eq!(first_key.len(), THEME_KEY_PREFIX.len() + 32);
        assert!(first_key.starts_with(THEME_KEY_PREFIX));
        assert_eq!(&first_key[THEME_KEY_PREFIX.len()..], &first);
        assert_ne!(first_key, theme_key(&second));
        assert_ne!(first_key, ACCOUNT_SEED_KEY);
        assert_ne!(first_key, NICKNAME_KEY);
    }

    #[test]
    fn stored_themes_round_trip_and_reject_corruption() {
        let theme = Theme {
            piece_set: PieceSet::Classic,
            white_piece: "#aAbBcC".into(),
            ..Theme::default()
        };
        let encoded = encode_theme(&theme).unwrap();
        assert_eq!(decode_theme(&encoded).unwrap(), theme);
        assert!(decode_theme(&[]).is_err());
        assert!(decode_theme(&encoded[..encoded.len() - 1]).is_err());
        let mut with_trailing_data = encoded;
        with_trailing_data.push(0);
        assert!(decode_theme(&with_trailing_data).is_err());

        let invalid = Theme {
            black_piece: "red".into(),
            ..theme
        };
        assert!(encode_theme(&invalid).is_err());
        let mut invalid_bytes = Vec::new();
        ciborium::ser::into_writer(&invalid, &mut invalid_bytes).unwrap();
        assert!(decode_theme(&invalid_bytes).is_err());
    }

    #[test]
    fn missing_themes_and_failed_writes_are_distinct() {
        // Native DelegateCtx intentionally has no host store: reads miss and writes fail.
        let mut ctx = DelegateCtx::default();
        let player = [3; 32];
        assert_eq!(
            handle(&mut ctx, ChessDelegateRequest::GetTheme { player }),
            ChessDelegateResponse::Theme {
                player,
                theme: None
            }
        );
        assert_eq!(
            handle(
                &mut ctx,
                ChessDelegateRequest::SetTheme {
                    player,
                    theme: Theme::default()
                }
            ),
            ChessDelegateResponse::Error {
                message: "the secret store rejected the theme".into()
            }
        );
        let invalid = Theme {
            light_square: "red".into(),
            ..Theme::default()
        };
        assert_eq!(
            handle(
                &mut ctx,
                ChessDelegateRequest::SetTheme {
                    player,
                    theme: invalid
                }
            ),
            ChessDelegateResponse::Error {
                message: "theme colors must be #RRGGBB hex values".into()
            }
        );
    }
}
