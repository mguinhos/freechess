//! The chess delegate: durable, device-local storage for the player's account.
//!
//! Everything else in FreeChess is public replicated state. This is the
//! exception — it runs inside the user's own Freenet core, its secret store
//! never leaves the device, and it is what lets a player close the browser and
//! come back as the same person.
//!
//! See [`chess_core::delegate_api`] for the protocol and for why the seed is
//! handed back to the UI rather than the delegate signing on its behalf.

#![allow(unexpected_cfgs)]

use chess_core::delegate_api::{
    ChessDelegateRequest, ChessDelegateResponse, ACCOUNT_SEED_KEY, NICKNAME_KEY,
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
                ChessDelegateResponse::Stored
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
    }
}
