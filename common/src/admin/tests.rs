//! Administration tests.
//!
//! The two things that matter: only real admins can act, and the root race
//! converges — every peer must agree on who is admin from the same set of
//! grants, in any order.

use super::*;
use crate::identity::GameId;
use crate::lobby::{LobbyParametersV1, LobbyStateV1};
use ed25519_dalek::SigningKey;
use freenet_scaffold::ComposableState;
use rand::rngs::OsRng;

const T0: i64 = 1_700_000_000_000;

fn key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

fn params() -> LobbyParametersV1 {
    LobbyParametersV1::default()
}

/// Apply actions through the real merge path.
fn apply(admin: &mut AdministrationV1, actions: Vec<AdminAction>) -> Result<(), String> {
    let parent = LobbyStateV1::default();
    admin.apply_delta(&parent, &params(), &Some(actions))?;
    admin.prune();
    Ok(())
}

// -------------------------------------------------------------- claiming

#[test]
fn the_first_claim_becomes_admin() {
    let alice = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &alice,
            "alice".to_string(),
            T0,
        ))],
    )
    .unwrap();

    assert!(admin.is_admin(PlayerId::from(&alice.verifying_key())));
    admin.check().expect("a single claim is well-formed");
}

#[test]
fn a_second_self_claim_cannot_create_a_rival_root() {
    let alice = key();
    let bob = key();
    let mut admin = AdministrationV1::default();

    // Alice claims first; Bob claims later. Only one root may survive.
    apply(
        &mut admin,
        vec![
            AdminAction::Grant(AdminGrant::claim(&alice, "alice".to_string(), T0)),
            AdminAction::Grant(AdminGrant::claim(&bob, "bob".to_string(), T0 + 5000)),
        ],
    )
    .unwrap();

    assert!(admin.is_admin(PlayerId::from(&alice.verifying_key())));
    assert!(
        !admin.is_admin(PlayerId::from(&bob.verifying_key())),
        "the later claim must lose"
    );
    admin.check().unwrap();
}

#[test]
fn the_claim_race_resolves_identically_in_any_order() {
    let alice = key();
    let bob = key();
    let a = AdminAction::Grant(AdminGrant::claim(&alice, "alice".to_string(), T0));
    let b = AdminAction::Grant(AdminGrant::claim(&bob, "bob".to_string(), T0 + 5000));

    let mut forward = AdministrationV1::default();
    apply(&mut forward, vec![a.clone(), b.clone()]).unwrap();
    let mut backward = AdministrationV1::default();
    apply(&mut backward, vec![b, a]).unwrap();

    assert_eq!(forward, backward, "the root race must converge");
}

#[test]
fn merging_the_same_claim_twice_changes_nothing() {
    let alice = key();
    let claim = AdminAction::Grant(AdminGrant::claim(&alice, "alice".to_string(), T0));
    let mut admin = AdministrationV1::default();
    apply(&mut admin, vec![claim.clone()]).unwrap();
    let once = admin.clone();
    apply(&mut admin, vec![claim]).unwrap();
    assert_eq!(once, admin, "merge must be idempotent");
}

// -------------------------------------------------------------- granting

#[test]
fn an_admin_can_promote_someone_and_they_can_promote_further() {
    let root = key();
    let second = key();
    let third = key();
    let mut admin = AdministrationV1::default();

    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::grant(
            &root,
            second.verifying_key(),
            "second".to_string(),
            T0 + 1000,
        ))],
    )
    .unwrap();
    // The chain extends: an admin created by an admin can create another.
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::grant(
            &second,
            third.verifying_key(),
            "third".to_string(),
            T0 + 2000,
        ))],
    )
    .unwrap();

    assert!(admin.is_admin(PlayerId::from(&second.verifying_key())));
    assert!(admin.is_admin(PlayerId::from(&third.verifying_key())));
    assert_eq!(admin.admins.len(), 3);
    admin.check().unwrap();
}

#[test]
fn a_non_admin_cannot_promote_anyone() {
    let root = key();
    let outsider = key();
    let target = key();
    let mut admin = AdministrationV1::default();

    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    // A perfectly well-signed grant — from someone with no authority.
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::grant(
            &outsider,
            target.verifying_key(),
            "sneaky".to_string(),
            T0 + 1000,
        ))],
    )
    .unwrap();

    assert!(!admin.is_admin(PlayerId::from(&target.verifying_key())));
    assert_eq!(admin.admins.len(), 1);
}

#[test]
fn a_grant_with_a_forged_signature_is_rejected() {
    let root = key();
    let impostor = key();
    let target = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    // Claim the root granted it, but sign with a different key.
    let mut forged = AdminGrant::grant(&impostor, target.verifying_key(), "x".to_string(), T0 + 1);
    forged.granted_by = root.verifying_key();

    let err = apply(&mut admin, vec![AdminAction::Grant(forged)]).expect_err("must reject");
    assert!(err.contains("invalid signature"), "got: {err}");
}

#[test]
fn a_full_state_put_of_self_appointed_admins_is_rejected() {
    // The merge path prunes unauthorised grants, but a full-state PUT skips it,
    // so `check` has to refuse on its own.
    let alice = key();
    let bob = key();
    let mut admin = AdministrationV1::default();
    for k in [&alice, &bob] {
        let g = AdminGrant::claim(k, "x".to_string(), T0);
        admin.admins.grants.insert(g.grantee_id(), g);
    }
    let err = admin.check().expect_err("must reject");
    assert!(err.contains("more than one root"), "got: {err}");
}

// ---------------------------------------------------------- announcements

#[test]
fn only_an_admin_can_publish_an_announcement() {
    let root = key();
    let outsider = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    apply(
        &mut admin,
        vec![AdminAction::Announce(Announcement::new(
            &outsider,
            "spam".to_string(),
            T0 + 1,
        ))],
    )
    .unwrap();
    assert!(admin.announcements.is_empty(), "outsider must be ignored");

    apply(
        &mut admin,
        vec![AdminAction::Announce(Announcement::new(
            &root,
            "scheduled maintenance".to_string(),
            T0 + 2,
        ))],
    )
    .unwrap();
    assert_eq!(admin.announcements.len(), 1);
    assert_eq!(
        admin.recent_announcements()[0].text,
        "scheduled maintenance"
    );
    admin.check().unwrap();
}

#[test]
fn a_tampered_announcement_is_rejected() {
    let root = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    let mut a = Announcement::new(&root, "original".to_string(), T0 + 1);
    a.text = "rewritten".to_string();
    apply(&mut admin, vec![AdminAction::Announce(a)]).unwrap();
    assert!(admin.announcements.is_empty());
}

#[test]
fn announcements_are_capped_deterministically() {
    let root = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    let actions: Vec<AdminAction> = (0..(MAX_ANNOUNCEMENTS + 10))
        .map(|i| {
            AdminAction::Announce(Announcement::new(
                &root,
                format!("notice {i}"),
                T0 + 1000 + i as i64,
            ))
        })
        .collect();
    let mut reversed = actions.clone();
    reversed.reverse();

    apply(&mut admin, actions).unwrap();
    assert_eq!(admin.announcements.len(), MAX_ANNOUNCEMENTS);

    let mut other = AdministrationV1::default();
    apply(
        &mut other,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    apply(&mut other, reversed).unwrap();
    assert_eq!(admin, other, "the cap must not depend on arrival order");
}

// -------------------------------------------------------------- takedowns

#[test]
fn only_an_admin_can_take_a_game_down() {
    let root = key();
    let outsider = key();
    let game = GameId([9u8; 32]);
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    apply(
        &mut admin,
        vec![AdminAction::TakeDown(Takedown::new(
            &outsider,
            game,
            "because".to_string(),
            T0 + 1,
        ))],
    )
    .unwrap();
    assert!(!admin.is_taken_down(&game));

    apply(
        &mut admin,
        vec![AdminAction::TakeDown(Takedown::new(
            &root,
            game,
            "abuse".to_string(),
            T0 + 2,
        ))],
    )
    .unwrap();
    assert!(admin.is_taken_down(&game));
    admin.check().unwrap();
}

// ---------------------------------------------------------- service state

#[test]
fn an_admin_can_mark_the_service_unavailable() {
    let root = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    assert!(admin.available(), "available by default");

    apply(
        &mut admin,
        vec![AdminAction::Service(ServiceState::new(
            &root,
            false,
            "down for maintenance".to_string(),
            T0 + 1,
        ))],
    )
    .unwrap();
    assert!(!admin.available());
    assert_eq!(
        admin.service_message().as_deref(),
        Some("down for maintenance")
    );

    // And back up again: the newest statement wins.
    apply(
        &mut admin,
        vec![AdminAction::Service(ServiceState::new(
            &root,
            true,
            String::new(),
            T0 + 2,
        ))],
    )
    .unwrap();
    assert!(admin.available());
    admin.check().unwrap();
}

#[test]
fn a_non_admin_cannot_change_service_state() {
    let root = key();
    let outsider = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    apply(
        &mut admin,
        vec![AdminAction::Service(ServiceState::new(
            &outsider,
            false,
            "hijacked".to_string(),
            T0 + 1,
        ))],
    )
    .unwrap();
    assert!(
        admin.available(),
        "outsider must not be able to shut it down"
    );
}

// ------------------------------------------------- losing the root race

#[test]
fn grants_from_a_root_that_loses_the_race_are_revoked() {
    // Bob claims and promotes Carol. Alice's earlier claim then surfaces and
    // wins the root race, so Bob's whole branch — including Carol and anything
    // Bob published — must fall away.
    let alice = key();
    let bob = key();
    let carol = key();

    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![
            AdminAction::Grant(AdminGrant::claim(&bob, "bob".to_string(), T0 + 5000)),
            AdminAction::Grant(AdminGrant::grant(
                &bob,
                carol.verifying_key(),
                "carol".to_string(),
                T0 + 6000,
            )),
        ],
    )
    .unwrap();
    apply(
        &mut admin,
        vec![AdminAction::Announce(Announcement::new(
            &bob,
            "bob was here".to_string(),
            T0 + 7000,
        ))],
    )
    .unwrap();
    assert_eq!(admin.admins.len(), 2);
    assert_eq!(admin.announcements.len(), 1);

    // Alice's earlier claim arrives.
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &alice,
            "alice".to_string(),
            T0,
        ))],
    )
    .unwrap();

    assert!(admin.is_admin(PlayerId::from(&alice.verifying_key())));
    assert!(!admin.is_admin(PlayerId::from(&bob.verifying_key())));
    assert!(!admin.is_admin(PlayerId::from(&carol.verifying_key())));
    assert!(
        admin.announcements.is_empty(),
        "content from a revoked admin must go too"
    );
    admin.check().unwrap();
}

#[test]
fn prune_is_idempotent() {
    let root = key();
    let second = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![
            AdminAction::Grant(AdminGrant::claim(&root, "root".to_string(), T0)),
            AdminAction::Grant(AdminGrant::grant(
                &root,
                second.verifying_key(),
                "second".to_string(),
                T0 + 1,
            )),
            AdminAction::Announce(Announcement::new(&root, "hello".to_string(), T0 + 2)),
        ],
    )
    .unwrap();

    let once = admin.clone();
    admin.prune();
    assert_eq!(once, admin);
}

#[test]
fn peers_holding_different_announcements_exchange_them() {
    // The summary was a triple of collection *lengths*. Two peers each holding
    // one different announcement therefore summarized identically, neither
    // shipped a delta, and the divergence was permanent — the same for a grant
    // each and a takedown each.
    let root = key();
    let claim = AdminAction::Grant(AdminGrant::claim(&root, "root".to_string(), T0));

    let mut peer1 = AdministrationV1::default();
    apply(&mut peer1, vec![claim.clone()]).unwrap();
    let mut peer2 = peer1.clone();

    apply(
        &mut peer1,
        vec![AdminAction::Announce(Announcement::new(
            &root,
            "first notice".to_string(),
            T0 + 1_000,
        ))],
    )
    .unwrap();
    apply(
        &mut peer2,
        vec![AdminAction::Announce(Announcement::new(
            &root,
            "second notice".to_string(),
            T0 + 2_000,
        ))],
    )
    .unwrap();

    assert_eq!(peer1.announcements.len(), peer2.announcements.len());
    assert_ne!(peer1.announcements, peer2.announcements);

    let parent = LobbyStateV1::default();
    let s1 = peer1.summarize(&parent, &params());
    let d = peer2
        .delta(&parent, &params(), &s1)
        .expect("a differing announcement set must ship");
    peer1.apply_delta(&parent, &params(), &Some(d)).unwrap();
    peer1.prune();

    let s2 = peer2.summarize(&parent, &params());
    if let Some(d) = peer1.delta(&parent, &params(), &s2) {
        peer2.apply_delta(&parent, &params(), &Some(d)).unwrap();
        peer2.prune();
    }

    assert_eq!(peer1.announcements.len(), 2);
    assert_eq!(peer1.announcements, peer2.announcements);
}

// ------------------------------------------------------------- migration

/// A plausible-looking contract id: 32 bytes, base58. The notice refuses
/// anything else so a client can render it as a link without sanitising.
fn an_address() -> String {
    GameId([7u8; 32]).to_base58()
}

#[test]
fn only_an_admin_may_announce_a_migration() {
    let root = key();
    let stranger = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    let forged = Migration::announce(&stranger, an_address(), "moved".to_string(), T0 + 1000);
    let err = forged.verify(&admin.admins).expect_err("must reject");
    assert!(err.contains("only an admin"), "got: {err}");

    // And the merge path skips it rather than accepting it.
    apply(&mut admin, vec![AdminAction::Migrate(forged)]).unwrap();
    assert!(admin.migration().is_none(), "a stranger seated a migration");
    assert!(admin.accepting_new_games());
}

#[test]
fn a_migration_address_must_be_a_real_contract_id() {
    let root = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();

    let junk = Migration::announce(
        &root,
        "javascript:alert(1)".to_string(),
        "moved".to_string(),
        T0 + 1000,
    );
    let err = junk.verify(&admin.admins).expect_err("must reject");
    assert!(err.contains("valid contract id"), "got: {err}");
}

#[test]
fn a_migration_can_be_announced_and_called_off() {
    let root = key();
    let mut admin = AdministrationV1::default();
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    assert!(admin.accepting_new_games());

    let notice = Migration::announce(&root, an_address(), "we moved".to_string(), T0 + 1000);
    apply(&mut admin, vec![AdminAction::Migrate(notice)]).unwrap();
    assert_eq!(admin.migration().unwrap().new_address, an_address());
    assert!(!admin.accepting_new_games(), "new games must be locked");

    // A later cancellation reopens the lobby.
    let cancel = Migration::cancel(&root, T0 + 2000);
    apply(&mut admin, vec![AdminAction::Migrate(cancel)]).unwrap();
    assert!(admin.migration().is_none());
    assert!(admin.accepting_new_games());
}

/// Two admins announcing at once must not split the network.
#[test]
fn competing_migration_notices_converge() {
    let root = key();
    let second = key();
    let mut base = AdministrationV1::default();
    apply(
        &mut base,
        vec![AdminAction::Grant(AdminGrant::claim(
            &root,
            "root".to_string(),
            T0,
        ))],
    )
    .unwrap();
    apply(
        &mut base,
        vec![AdminAction::Grant(AdminGrant::grant(
            &root,
            second.verifying_key(),
            "second".to_string(),
            T0 + 100,
        ))],
    )
    .unwrap();

    let a = AdminAction::Migrate(Migration::announce(
        &root,
        an_address(),
        "a".to_string(),
        T0 + 1000,
    ));
    let b = AdminAction::Migrate(Migration::announce(
        &second,
        GameId([9u8; 32]).to_base58(),
        "b".to_string(),
        T0 + 1000,
    ));

    let mut peer1 = base.clone();
    apply(&mut peer1, vec![a.clone()]).unwrap();
    apply(&mut peer1, vec![b.clone()]).unwrap();

    let mut peer2 = base;
    apply(&mut peer2, vec![b]).unwrap();
    apply(&mut peer2, vec![a]).unwrap();

    assert_eq!(peer1.migration, peer2.migration, "must converge");
    assert!(peer1.migration().is_some());
}

/// A migration published by an admin whose grant later loses the root race
/// goes with it, like every other action that admin took.
#[test]
fn a_revoked_admins_migration_is_dropped() {
    let alice = key();
    let bob = key();
    let mut admin = AdministrationV1::default();

    // Bob claims later, so his branch loses once Alice's claim is merged.
    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &bob,
            "bob".to_string(),
            T0 + 5000,
        ))],
    )
    .unwrap();
    let notice = Migration::announce(&bob, an_address(), "moved".to_string(), T0 + 6000);
    apply(&mut admin, vec![AdminAction::Migrate(notice)]).unwrap();
    assert!(admin.migration().is_some(), "premise: bob's notice is live");

    apply(
        &mut admin,
        vec![AdminAction::Grant(AdminGrant::claim(
            &alice,
            "alice".to_string(),
            T0,
        ))],
    )
    .unwrap();
    assert!(
        admin.migration().is_none(),
        "the notice outlived the admin who signed it"
    );
    assert!(admin.accepting_new_games());
}
