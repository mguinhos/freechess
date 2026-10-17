//! Private per-account appearance settings stored by the local delegate.

pub use chess_delegate_protocol::{PieceSet, Theme};

pub fn style(theme: &Theme) -> String {
    if !theme.valid() {
        return style(&Theme::default());
    }
    format!(
        "--light-sq:{};--dark-sq:{};--white-piece:{};--black-piece:{};",
        theme.light_square, theme.dark_square, theme.white_piece, theme.black_piece
    )
}

pub fn receive(state: &mut crate::app::AppState, player: [u8; 32], theme: Option<Theme>) {
    if !state.account_settled
        || state.account.key.verifying_key().to_bytes() != player
        || state.theme_player.is_some()
        || theme.as_ref().is_some_and(|theme| !theme.valid())
    {
        return;
    }
    state.theme_player = Some(player);
    state.stored_theme = theme;
    state.theme_error = None;
}

pub fn saved(state: &mut crate::app::AppState, player: [u8; 32], theme: Theme) {
    if !state.account_settled
        || state.account.key.verifying_key().to_bytes() != player
        || state.pending_theme.as_ref() != Some(&theme)
        || !theme.valid()
    {
        return;
    }
    state.stored_theme = Some(theme);
    state.pending_theme = None;
    state.theme_error = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_replies_only_apply_to_the_settled_account() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let player = key.verifying_key().to_bytes();
        let mut state = crate::app::AppState::new(crate::identity::Account {
            key,
            nickname: "player".into(),
        });
        receive(&mut state, player, Some(Theme::default()));
        assert!(state.theme_player.is_none());
        state.account_settled = true;
        receive(&mut state, [1; 32], Some(Theme::default()));
        assert!(state.theme_player.is_none());
        let invalid = Theme {
            light_square: "red".into(),
            ..Theme::default()
        };
        receive(&mut state, player, Some(invalid));
        assert!(state.theme_player.is_none());
        receive(&mut state, player, Some(Theme::default()));
        assert_eq!(state.theme_player, Some(player));
        assert_eq!(state.stored_theme, Some(Theme::default()));
        let custom = Theme {
            dark_square: "#112233".into(),
            ..Theme::default()
        };
        state.pending_theme = Some(custom.clone());
        saved(&mut state, player, Theme::default());
        assert_eq!(state.pending_theme, Some(custom.clone()));
        saved(&mut state, player, custom.clone());
        assert!(state.pending_theme.is_none());
        receive(&mut state, player, None);
        saved(&mut state, player, Theme::default());
        assert_eq!(state.stored_theme, Some(custom));
    }

    #[test]
    fn css_rejects_untrusted_colors() {
        let theme = Theme {
            white_piece: "#123456;background:red".into(),
            ..Theme::default()
        };
        assert_eq!(style(&theme), style(&Theme::default()));
    }
}
