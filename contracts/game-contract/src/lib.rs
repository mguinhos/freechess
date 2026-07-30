//! One chess game: signed moves, the sealed opponent seat, and the result.
//!
//! The whole body is shared plumbing — see `chess_contract_support`. All the
//! rules that decide what a peer accepts live in the state type's `verify`,
//! `apply_delta` and `prune`, in `chess_core`, so they run identically here
//! and in the browser.

use chess_core::ChessGameParametersV1 as ContractParams;
use chess_core::ChessGameStateV1 as ContractState;
use freenet_stdlib::prelude::*;

#[allow(dead_code)]
struct Contract;

#[contract]
impl ContractInterface for Contract {
    fn validate_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        _related: RelatedContracts<'static>,
    ) -> Result<ValidateResult, ContractError> {
        chess_contract_support::validate::<ContractState, ContractParams>(parameters, state)
    }

    fn update_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        chess_contract_support::update::<ContractState, ContractParams>(parameters, state, data)
    }

    fn summarize_state(
        parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        chess_contract_support::summarize::<ContractState, ContractParams>(parameters, state)
    }

    fn get_state_delta(
        parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        chess_contract_support::get_delta::<ContractState, ContractParams>(
            parameters, state, summary,
        )
    }
}
